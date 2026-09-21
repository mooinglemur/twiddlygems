#!/usr/bin/env node
// Renders the synthesized sounds offline and measures them.
//
// Web Audio only exists in a browser, so this drives a headless one, but it
// renders through an OfflineAudioContext rather than a sound card, which makes
// the result exact and repeatable. The graph is the same one the game plays.
//
//   make audio

import { spawn } from 'node:child_process';

const PORT = Number(process.env.AUDIO_PORT ?? 8105);
const DEBUG_PORT = Number(process.env.AUDIO_DEBUG_PORT ?? 9446);
const BROWSER = process.env.BROWSER ?? 'google-chrome-stable';
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const server = spawn(
  'python3',
  ['-m', 'http.server', String(PORT), '--bind', '127.0.0.1', '--directory', 'web'],
  { stdio: 'ignore' },
);
const browser = spawn(
  BROWSER,
  [
    // Muted at the device, which costs this check nothing: an
    // OfflineAudioContext renders into a buffer and never reaches a speaker.
    // The page behind it is the real game, though, and that one would play.
    '--headless', '--no-sandbox', '--disable-gpu', '--mute-audio',
    `--remote-debugging-port=${DEBUG_PORT}`,
    '--user-data-dir=/tmp/twiddlygems-audio',
    'about:blank',
  ],
  { stdio: 'ignore' },
);
browser.on('error', (error) => {
  console.error(`could not start ${BROWSER}: ${error.message}`);
  process.exit(1);
});
// Both children are killed by handle, never by name.
const stop = () => {
  browser.kill();
  server.kill();
};
process.on('exit', stop);

for (let i = 0; i < 120; i += 1) {
  try {
    if ((await fetch(`http://127.0.0.1:${DEBUG_PORT}/json/version`)).ok) break;
  } catch {}
  await sleep(100);
}
const target = await (
  await fetch(`http://127.0.0.1:${DEBUG_PORT}/json/new?about:blank`, { method: 'PUT' })
).json();
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.onopen = resolve;
  socket.onerror = reject;
});

let nextId = 1;
const pending = new Map();
socket.addEventListener('message', (message) => {
  const data = JSON.parse(message.data);
  if (data.id && pending.has(data.id)) {
    const { resolve, reject } = pending.get(data.id);
    pending.delete(data.id);
    if (data.error) reject(new Error(JSON.stringify(data.error)));
    else resolve(data.result);
  }
});
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });

await send('Page.enable');
await send('Runtime.enable');
await send('Page.navigate', { url: `http://127.0.0.1:${PORT}/index.html` });
await sleep(1500);

const { result } = await send('Runtime.evaluate', {
  awaitPromise: true,
  returnByValue: true,
  expression: `
  (async () => {
    const { Audio } = await import('./js/audio.js');
    const { SOUNDS, RIFFLE } = await import('./js/sounds.js');
    const RATE = 48000;

    // Renders \`count\` copies of a sound fired at once, through the real chain.
    async function render(name, count, seconds = 1, phoneFilter = false, opts = {}, bank = SOUNDS) {
      const ctx = new OfflineAudioContext(2, RATE * seconds, RATE);
      const audio = new Audio(bank);
      audio.attach(ctx);
      // Measured before the limiter, always.
      //
      // Two reasons. The question these levels answer is whether a sound
      // reaches the limiter at all, which is a question about what arrives at
      // it. And a DynamicsCompressorNode begins an offline render deep in gain
      // reduction, recovering over about a tenth of a second, so anything
      // starting at the top of a render came back five times too quiet, an
      // artifact of the measurement that does not happen in a context that has
      // been running.
      if (opts.keepLimiter !== true) {
        audio.master.disconnect();
        audio.master.connect(ctx.destination);
      }
      if (phoneFilter) {
        // Roughly what a phone speaker throws away: it cannot move air much
        // below 200Hz, so anything under that never reaches the player.
        const cut = ctx.createBiquadFilter();
        cut.type = 'highpass';
        cut.frequency.value = 200;
        cut.Q.value = 0.7;
        audio.master.disconnect();
        audio.master.connect(cut);
        cut.connect(ctx.destination);
      }
      for (let i = 0; i < count; i += 1) {
        audio.play(name, { pan: (i / Math.max(1, count - 1)) * 1.1 - 0.55, ...opts });
      }
      const buffer = await ctx.startRendering();
      const left = buffer.getChannelData(0);
      const right = buffer.getChannelData(1);

      let peak = 0;
      for (let i = 0; i < left.length; i += 1) {
        peak = Math.max(peak, Math.abs(left[i]), Math.abs(right[i]));
      }
      // Where a sound is judged to start and stop, relative to its own peak
      // rather than an absolute level. A fixed floor measures something
      // different at every volume: change a gain anywhere and every duration
      // moves with it, which is the measurement talking, not the sound.
      const floor = Math.max(1e-6, peak * 0.004);

      let sum = 0, last = 0, first = -1, crossings = 0, previous = 0;
      for (let i = 0; i < left.length; i += 1) {
        const v = Math.max(Math.abs(left[i]), Math.abs(right[i]));
        sum += left[i] * left[i];
        if (v > floor) {
          last = i;
          if (first < 0) first = i;
        }
        const mono = left[i] + right[i];
        if ((mono > 0 && previous <= 0) || (mono < 0 && previous >= 0)) crossings += 1;
        previous = mono;
      }

      // Brightness at the start against brightness at the end. A sound whose
      // filter sweeps down gets darker as it fades; one that simply stops does
      // not. This is what separates a poof from a click with a long tail.
      const band = (from, to) => {
        let n = 0, prev = 0;
        for (let i = from; i < to; i += 1) {
          const mono = left[i] + right[i];
          if ((mono > 0 && prev <= 0) || (mono < 0 && prev >= 0)) n += 1;
          prev = mono;
        }
        return Math.round(n / (Math.max(1, to - from) / RATE));
      };
      const third = Math.max(1, Math.floor(last / 3));

      // How many separate strikes the sound contains, counted as rising edges
      // of a short-window envelope. One knock or two is audible at a glance but
      // not otherwise checkable, and "two rapid hits" is the whole brief.
      let hitEnvelope = [];
      const hits = (() => {
        const win = Math.max(1, Math.floor(RATE * 0.003));
        const envelope = [];
        for (let i = 0; i < last; i += win) {
          let loudest = 0;
          for (let j = i; j < Math.min(last, i + win); j += 1) {
            loudest = Math.max(loudest, Math.abs(left[j]) + Math.abs(right[j]));
          }
          envelope.push(loudest);
        }
        const ceiling = Math.max(...envelope, 1e-9);
        hitEnvelope = envelope.map((v) => Number((v / ceiling).toFixed(2)));
        let count = 0;
        let above = false;
        for (const level of envelope) {
          const loud = level > ceiling * 0.35;
          if (loud && !above) {
            count += 1;
          }
          above = loud;
        }
        return count;
      })();

      // How steadily the sound holds its pitch.
      //
      // Measured cycle by cycle rather than by counting crossings in a window:
      // the gap between successive upward zero crossings is the instantaneous
      // period, which resolves a few percent of pitch change. Each period is
      // compared against a wide average of its neighbours, so a steady glide
      // reads near zero however fast it falls.
      const wobble = (() => {
        // Crossings are interpolated between samples. Snapping them to whole
        // samples sounds harmless, but a period at these pitches is only about
        // thirty samples long, so rounding alone adds over a percent of noise,
        // the same size as the waver being measured, and enough to bury it.
        const marks = [];
        let was = 0;
        for (let i = third; i < last; i += 1) {
          const v = left[i] + right[i];
          if (v > 0 && was <= 0 && v !== was) {
            marks.push(i - 1 + -was / (v - was));
          }
          was = v;
        }
        const periods = [];
        for (let i = 1; i < marks.length; i += 1) {
          periods.push(marks[i] - marks[i - 1]);
        }
        // Each period is compared against the midpoint of two far neighbours
        // rather than an average of everything between. Averaging a window
        // narrower than a few waver steps partly follows the waver and hides
        // it; taking the midpoint of the endpoints cancels the glide's local
        // slope instead, and leaves the wobble behind.
        const REACH = 110;
        if (periods.length < REACH * 2 + 3) return 0;
        let deviation = 0, counted = 0;
        for (let i = REACH; i < periods.length - REACH; i += 1) {
          const expected = (periods[i - REACH] + periods[i + REACH]) / 2;
          if (expected > 0) {
            deviation += Math.abs(periods[i] - expected) / expected;
            counted += 1;
          }
        }
        return counted ? Number((deviation / counted).toFixed(4)) : 0;
      })();

      return {
        peak: Number(peak.toFixed(4)),
        // Averaged over the sound's own extent, not the whole render. Dividing
        // by the buffer made a long tone in a long render look quieter than a
        // tick in a short one, which is a property of the measurement and not
        // of the sound.
        rms: Number(Math.sqrt(sum / Math.max(1, last - Math.max(0, first))).toFixed(5)),
        // Over the whole render instead, for comparing two renders of the same
        // length with each other. The extent-based figure above shifts when a
        // filter shortens the tail, which would make a comparison of the two
        // measure the window rather than the sound.
        rmsBuffer: Number(Math.sqrt(sum / left.length).toFixed(6)),
        ms: Number(((last / RATE) * 1000).toFixed(1)),
        // When it actually began, which is what a scatter moves around.
        onsetMs: Number(((Math.max(0, first) / RATE) * 1000).toFixed(1)),
        hits,
        envelope: hitEnvelope,
        // A rough brightness proxy: noisy, hi-hat-like sounds cross zero often.
        zcrPerSec: Math.round(crossings / (Math.max(1, last) / RATE)),
        wobble,
        early: band(0, third),
        late: band(third * 2, last),
        voices: audio.started,
      };
    }

    // The loudest of several renders of the same stack.
    //
    // A stack's peak is a random variable, not a property of the sound. Copies
    // are detuned per play, so they drift in and out of phase with each other
    // and how squarely they happen to line up decides the peak: nine thuds
    // measured anywhere between 0.33 and 0.91 at one fixed gain. A single
    // render samples that at random, which is enough to fail the build on a
    // Tuesday and pass it on a Wednesday, and enough to argue a level down
    // that never needed touching.
    //
    // What the threshold checks is whether a stack *can* clip, so the worst of
    // several runs is the honest number rather than any one of them.
    async function worst(name, count, seconds = 1, runs = 8, opts = {}) {
      let loudest = null;
      for (let i = 0; i < runs; i += 1) {
        const take = await render(name, count, seconds, false, opts);
        if (!loudest || take.peak > loudest.peak) {
          loudest = take;
        }
      }
      return loudest;
    }

    // Same sound, no per-play randomness, so the two renders are comparable.
    const steadyBoom = JSON.parse(JSON.stringify(SOUNDS.boom));
    for (const layer of steadyBoom.layers) delete layer.jitter;
    const plain = { boom: steadyBoom };
    const full = await render('boom', 1, 1.2, false, {}, plain);
    const thin = await render('boom', 1, 1.2, true, {}, plain);

    // The thud asks the same question as the boom does: it is pitched low
    // enough that a phone could be handed most of it and play none of it.
    const steadyThud = JSON.parse(JSON.stringify(SOUNDS.thud));
    for (const layer of steadyThud.layers) delete layer.jitter;
    const thudBank = { thud: steadyThud };
    const thudFull = await render('thud', 1, 0.6, false, {}, thudBank);
    const thudThin = await render('thud', 1, 0.6, true, {}, thudBank);

    // A control for the shuffle's rattle: its own strikes, taken from the
    // shipped sound, rendered as they ship and again all piled onto the same
    // instant. Only the spreading differs, so the difference is the riffle.
    // The jitter comes off both so the two are comparable.
    const rattle = (spread) => ({
      rattle: {
        gain: SOUNDS.shuffle.gain,
        layers: SOUNDS.shuffle.layers
          .filter((layer) => RIFFLE.some(([delay]) => layer.delay === delay))
          .map(({ jitter, ...layer }) => ({ ...layer, delay: spread ? layer.delay : 0 })),
      },
    });
    const rattleSpread = await render('rattle', 1, 1, false, {}, rattle(true));
    const rattleStacked = await render('rattle', 1, 1, false, {}, rattle(false));

    // A control for the wobble measurement: the same falling glide with and
    // without the waver, so the number means something.
    const glide = (waver) => ({
      tone: {
        gain: 0.4,
        duration: 1,
        layers: [
          {
            source: 'triangle',
            note: 2050,
            sweep: { to: 720, time: 0.86 },
            ...(waver ? { waver } : {}),
            env: { attack: 0.05, hold: 0.77, decay: 0.08 },
          },
        ],
      },
    });
    // The control uses whatever the shipped rocket uses, so the check is about
    // the real sound rather than a number picked to pass.
    const shipped = SOUNDS.rocket.layers.find((l) => l.waver)?.waver ?? { depth: 0.05, rate: 17 };
    const steadyTone = await render('tone', 1, 1.5, false, {}, glide(false));
    const waveryTone = await render('tone', 1, 1.5, false, {}, glide(shipped));
    // Deliberately absurd, to tell a broken feature from an insensitive ruler.
    // The waver is a random walk, so one render is a noisy sample of it:
    // measured alone it ranges over a factor of five between runs, which is
    // enough to make a threshold fire at random. Averaged over several, it is
    // steady enough to assert on.
    const wildRuns = [];
    for (let i = 0; i < 5; i += 1) {
      wildRuns.push(await render('tone', 1, 1.5, false, {}, glide({ depth: 0.4, rate: 17 })));
    }
    const wildTone = {
      ...wildRuns[0],
      wobble: Number(
        (wildRuns.reduce((sum, r) => sum + r.wobble, 0) / wildRuns.length).toFixed(4),
      ),
    };
    return {
      one: await render('pop', 1),
      three: await worst('pop', 3),
      twenty: await render('pop', 20),
      boom: full,
      boomTwo: await worst('boom', 2, 1.2),
      boomFour: await render('boom', 4, 1.2),
      rocketShort: await render('rocket', 1, 2, false, { duration: 0.4 }),
      rocketLong: await render('rocket', 1, 2.5, false, { duration: 1.4 }),
      clack: await render('clack', 1, 0.6),
      shuffle: await render('shuffle', 1, 1.2),
      ding: await render('ding', 1, 1.6),
      rattleSpread,
      rattleStacked,
      thud: await render('thud', 1, 0.6),
      // Ordinary play: a row of three clears and those three columns settle.
      thudThree: await render('thud', 3, 0.6),
      // A board-wide collapse settles every column at once, which is the most
      // of these that can ever land together.
      thudBoard: await worst('thud', 9, 0.6),
      sparkle: await render('sparkle', 1, 2.6),
      sparkleTwenty: await worst('sparkle', 20, 2.6),
      // Several separate plays, to see whether the overtone really is redrawn.
      sparkleRuns: await Promise.all(
        [0, 1, 2, 3, 4, 5, 6, 7].map(() => render('sparkle', 1, 2.6)),
      ).then((runs) => ({
        pitches: runs.map((r) => r.early),
        onsets: runs.map((r) => r.onsetMs),
        peaks: runs.map((r) => r.peak),
      })),
      chimeFirst: await render('chime', 1, 0.8, false, { stage: 0 }),
      chimeLast: await render('chime', 1, 0.8, false, { stage: 11 }),
      // Past the end of the progression it should hold, not wrap round.
      chimePastEnd: await render('chime', 1, 0.8, false, { stage: 40 }),
      chimeStages: SOUNDS.chime.chords.length,
      // Read from the limiter itself, so this check cannot drift out of step
      // with the thing it is checking against.
      limiterDb: (() => {
        const probe = new Audio(SOUNDS);
        probe.attach(new OfflineAudioContext(2, 128, RATE));
        return probe.limiter.threshold.value;
      })(),
      sparkleScatterMs: (SOUNDS.sparkle.scatter ?? 0) * 1000,
      // How much of the boom survives a speaker that cannot do bass.
      boomThroughPhone: Number((thin.rmsBuffer / Math.max(1e-9, full.rmsBuffer)).toFixed(3)),
      thudThroughPhone: Number(
        (thudThin.rmsBuffer / Math.max(1e-9, thudFull.rmsBuffer)).toFixed(3),
      ),
      steadyTone,
      waveryTone,
      wildTone,
    };
  })()
  `,
});

if (result.subtype === 'error' || result.className === 'Error') {
  console.error('audio check failed:', result.description ?? JSON.stringify(result));
  stop();
  process.exit(1);
}

const stats = result.value;
// Where the limiter starts working, taken from the limiter rather than kept
// here as a second copy of the number.
const LIMITER_THRESHOLD = Number((10 ** (stats.limiterDb / 20)).toFixed(3));
console.log('rendered offline at 48kHz:');
for (const [label, key] of [
  ['pop x1', 'one'],
  ['pop x3', 'three'],
  ['pop x20', 'twenty'],
  ['boom', 'boom'],
  ['boom x2', 'boomTwo'],
  ['boom x4', 'boomFour'],
  ['rocket .4s', 'rocketShort'],
  ['rocket 1.4s', 'rocketLong'],
  ['clack', 'clack'],
  ['shuffle', 'shuffle'],
  ['ding', 'ding'],
  ['rattle out', 'rattleSpread'],
  ['rattle piled', 'rattleStacked'],
  ['thud', 'thud'],
  ['thud x3', 'thudThree'],
  ['thud x9', 'thudBoard'],
  ['sparkle', 'sparkle'],
  ['sparkle x20', 'sparkleTwenty'],
  ['chime 1/12', 'chimeFirst'],
  ['chime 12/12', 'chimeLast'],
  ['glide plain', 'steadyTone'],
  ['glide waver', 'waveryTone'],
  ['glide wild', 'wildTone'],
]) {
  const s = stats[key];
  console.log(
    `  ${label.padEnd(11)} peak ${String(s.peak).padEnd(7)} rms ${String(s.rms).padEnd(8)} ` +
      `from ${String(s.onsetMs).padStart(5)}ms  tail ${String(s.ms).padStart(6)}ms  ` +
      `bright ${String(s.early).padStart(5)} -> ` +
      `${String(s.late).padStart(5)}  wobble ${String(s.wobble).padEnd(6)} voices ${s.voices}`,
  );
}

// Twenty pops is a rainbow taking a whole color, and it does exceed the
// threshold; that one is the limiter's to catch. What must not is ordinary
// play, checked further down against the concurrency `make balance` measures.
const worst = stats.twenty;
if (stats.one.early <= stats.one.late) {
  console.error(
    `\nFAIL: the pop is as bright at the end (${stats.one.late}) as at the start ` +
      `(${stats.one.early}); its filter sweep is not working, so it will read as a click.`,
  );
  stop();
  process.exit(1);
}
if (stats.one.ms > 200) {
  console.error(`\nFAIL: a single pop rings for ${stats.one.ms}ms; it should be a short tick.`);
  stop();
  process.exit(1);
}
// What has to stay clear of the limiter is the sound designed to pile up.
// Twenty pops is an ordinary rainbow clear; two rocket impacts at once is
// already unusual, and four is what the limiter is for. Asking a rocket strike
// to be quiet enough that four of them never engage it would only make one of
// them thin. The x4 row is printed to show what the limiter is catching.
// The overtone has to actually be redrawn each play, or every gem rings the
// same partial and the "chord" is one note twenty times over.
const pitches = new Set(stats.sparkleRuns.pitches);
if (pitches.size < 3) {
  console.error(
    `\nFAIL: eight sparkles produced ${pitches.size} distinct pitches ` +
      `(${stats.sparkleRuns.pitches}). The harmonic is not being drawn per play.`,
  );
  stop();
  process.exit(1);
}

console.log(
  `\neight separate sparkles, peaks ${stats.sparkleRuns.peaks.join(', ')}\n` +
    `                       starts ${stats.sparkleRuns.onsets.map((m) => `${m}ms`).join(', ')}`,
);

// They also have to start at different moments, or a clear lands as one chime.
const onsets = stats.sparkleRuns.onsets;
const onsetRange = Math.max(...onsets) - Math.min(...onsets);
const declaredScatter = stats.sparkleScatterMs;
if (declaredScatter > 0 && onsetRange < declaredScatter * 0.3) {
  console.error(
    `\nFAIL: eight sparkles started within ${onsetRange.toFixed(0)}ms of each other, against a ` +
      `declared scatter of ${declaredScatter}ms (${onsets}). They are not being spread out.`,
  );
  stop();
  process.exit(1);
}

// Checked against what ordinary play actually asks for, not the extreme.
// `make balance` counts gems clearing within one frame of each other across
// whole playthroughs: three is the median and four the ninetieth percentile.
// Twenty is a rainbow taking a whole color, and letting that one meet the
// limiter is what the limiter is for, so its row is printed, not asserted.
// Every column landing at once is asserted rather than printed, unlike the
// extremes above: a landing per column is not a worst case a rare move reaches
// but what any board-wide collapse does, so the thud has to be quiet enough for
// the whole width of the board at once.
const loudest = Math.max(
  stats.three.peak,
  stats.boomTwo.peak,
  stats.sparkleTwenty.peak,
  stats.thudBoard.peak,
);
if (loudest >= LIMITER_THRESHOLD) {
  console.error(
    `\nFAIL: ordinary play is reaching the limiter. Three pops peak ${stats.three.peak}, ` +
      `twenty sparkles ${stats.sparkleTwenty.peak}, two booms ${stats.boomTwo.peak} and ` +
      `a board of thuds ${stats.thudBoard.peak}, against ${LIMITER_THRESHOLD}.`,
  );
  stop();
  process.exit(1);
}
// The thud has to stay underneath the gems rather than beside them: it fires
// while the pops of the clear that caused it are still ringing, and anything
// with the pop's brightness there would be heard as another gem going away.
if (stats.thud.early >= stats.one.early / 2) {
  console.error(
    `\nFAIL: the thud starts at ${stats.thud.early} against the pop's ${stats.one.early}; ` +
      `it is not low enough to read as a landing rather than another clear.`,
  );
  stop();
  process.exit(1);
}
if (stats.thud.ms > 250) {
  console.error(
    `\nFAIL: the thud runs ${stats.thud.ms}ms. Eight of these land together, so it has to be ` +
      `over before the next wave arrives.`,
  );
  stop();
  process.exit(1);
}
// Pitched this low, a thud can be sent to a phone and arrive as nothing at
// all. This is the same check the boom gets, for the same reason.
if (stats.thudThroughPhone < 0.25) {
  console.error(
    `\nFAIL: only ${(stats.thudThroughPhone * 100).toFixed(0)}% of the thud survives a 200Hz ` +
      `highpass. On a phone speaker the board would settle in silence.`,
  );
  stop();
  process.exit(1);
}
if (stats.boom.ms < 350 || stats.boom.ms > 700) {
  console.error(`\nFAIL: the boom runs ${stats.boom.ms}ms; it is meant to be about 500.`);
  stop();
  process.exit(1);
}
// Two things are checked about the waver, and one deliberately is not.
//
// That the mechanism works at all is checked against an exaggerated control,
// because a subtle waver is not something this ruler can resolve: at the depth
// actually shipped it measures barely above a plain glide, which turns out to
// be the floor of measuring pitch through zero crossings while the frequency
// is being automated. Asserting on the shipped depth would be asserting on
// noise.
//
// That it is not a wild vibrato is checked directly, because shipping one is a
// mistake that has already been made here once: 0.16 measured about 0.06 and
// was unmistakable.
//
// The depth itself is set by ear. No number here has an opinion about it.
const WAVER_CEILING = 0.045;
if (stats.wildTone.wobble < stats.steadyTone.wobble * 4) {
  console.error(
    `\nFAIL: the waver does nothing even at an absurd depth. A plain glide wobbles ` +
      `${stats.steadyTone.wobble} and one wavering by 40% only ${stats.wildTone.wobble}. ` +
      `The mechanism is broken.`,
  );
  stop();
  process.exit(1);
}
if (stats.waveryTone.wobble > WAVER_CEILING) {
  console.error(
    `\nFAIL: the shipped waver measures ${stats.waveryTone.wobble}, past ${WAVER_CEILING}. ` +
      `That is a vibrato, not a firework failing to hold its note.`,
  );
  stop();
  process.exit(1);
}

// The rejected-swap sound is two knocks, high then low. One knock is a
// different sound, and low-then-high is the wrong gesture.
if (stats.clack.hits !== 2) {
  console.error(
    `\nFAIL: the rejected-swap sound has ${stats.clack.hits} strike(s), not two.\n` +
      `  envelope (3ms per step, relative): ${stats.clack.envelope.join(' ')}`,
  );
  stop();
  process.exit(1);
}
// The low-moves bell is two notes, high then low. The strike counter is a fair
// ruler here, where it is not for the shuffle below: these two are far enough
// apart and even enough in level that it reads 2 every time.
if (stats.ding.hits !== 2) {
  console.error(
    `\nFAIL: the low-moves bell has ${stats.ding.hits} note(s), not two.\n` +
      `  envelope (3ms per step, relative): ${stats.ding.envelope.join(' ')}`,
  );
  stop();
  process.exit(1);
}
if (stats.ding.early <= stats.ding.late) {
  console.error(
    `\nFAIL: the low-moves bell runs low to high (${stats.ding.early} then ` +
      `${stats.ding.late}); a doorbell falls.`,
  );
  stop();
  process.exit(1);
}

// A riffle is many small strikes spread through time, not one wash of noise.
//
// Not counted with the strike counter that checks the clack's two knocks: the
// strikes here overlap and are jittered per play, so that counter reads
// anywhere from 5 to 10 for the real thing and 5 for a version with every
// strike stacked on one instant. It cannot tell them apart, so it is not asked
// to. Instead the same strikes are rendered twice, as they ship and all piled
// onto the same moment, and what separates a riffle from a thump is exactly
// the difference between those two.
if (stats.rattleSpread.ms < stats.rattleStacked.ms * 4) {
  console.error(
    `\nFAIL: spreading the shuffle's strikes out barely lengthens it ` +
      `(${stats.rattleSpread.ms}ms against ${stats.rattleStacked.ms}ms stacked). ` +
      `The rattle is landing as one thump.`,
  );
  stop();
  process.exit(1);
}
// And it darkens as it settles, like the rattle running out of energy.
if (stats.shuffle.early <= stats.shuffle.late) {
  console.error(
    `\nFAIL: the shuffle ends brighter than it starts (${stats.shuffle.early} then ` +
      `${stats.shuffle.late}); it should darken as the board settles.`,
  );
  stop();
  process.exit(1);
}

if (stats.clack.early <= stats.clack.late) {
  console.error(
    `\nFAIL: the rejected-swap sound runs low to high (${stats.clack.early} then ` +
      `${stats.clack.late}); it is meant to fall.`,
  );
  stop();
  process.exit(1);
}

// The chain's chords have to actually differ, or the progression is decoration
// on a sound that never changes. Higher chords cross zero more often, so the
// top of the scale must measurably outrank the bottom.
if (stats.chimeLast.early <= stats.chimeFirst.early * 1.4) {
  console.error(
    `\nFAIL: the last chord of the progression (${stats.chimeLast.early}) is no higher than ` +
      `the first (${stats.chimeFirst.early}). The stage is not selecting a chord.`,
  );
  stop();
  process.exit(1);
}
if (Math.abs(stats.chimePastEnd.early - stats.chimeLast.early) > stats.chimeLast.early * 0.25) {
  console.error(
    `\nFAIL: a stage past the end of the progression (${stats.chimePastEnd.early}) does not ` +
      `hold at the last chord (${stats.chimeLast.early}); a long chain would wrap round.`,
  );
  stop();
  process.exit(1);
}
// A loose sanity bound rather than a target. How long a chord rings is a
// musical decision made by ear and changed often; this only catches a note
// that has become a click or one that runs into the next clear.
if (stats.chimeFirst.ms < 80 || stats.chimeFirst.ms > 900) {
  console.error(
    `\nFAIL: a chord rings for ${stats.chimeFirst.ms}ms, which is not a plucked note.`,
  );
  stop();
  process.exit(1);
}

// A longer flight has to whistle for materially longer. Not in proportion,
// though: the ignition hiss is deliberately the same length however far the
// rocket is going, so on a short flight that fixed piece is most of the sound
// and the ratio comes in well under the one asked for.
const flightRatio = stats.rocketLong.ms / stats.rocketShort.ms;
if (flightRatio < 1.8 || flightRatio > 4.5) {
  console.error(
    `\nFAIL: asking for a 3.5x longer flight gave a ${flightRatio.toFixed(1)}x longer whistle.`,
  );
  stop();
  process.exit(1);
}
if (stats.boomThroughPhone < 0.25) {
  console.error(
    `\nFAIL: only ${(stats.boomThroughPhone * 100).toFixed(0)}% of the boom survives a 200Hz ` +
      `high-pass, so most of it is bass a phone cannot reproduce. Raise its pitch.`,
  );
  stop();
  process.exit(1);
}
console.log(
  `\naudio ok: loudest stack peaks ${loudest.toFixed(3)} (limiter at ${LIMITER_THRESHOLD}); ` +
    `the pop darkens ${stats.one.early} -> ${stats.one.late}; ` +
    `${(stats.boomThroughPhone * 100).toFixed(0)}% of the boom and ` +
    `${(stats.thudThroughPhone * 100).toFixed(0)}% of the thud survive a phone speaker; ` +
    `the waver works (${stats.steadyTone.wobble} plain, ${stats.wildTone.wobble} at 40% depth) ` +
    `and is not a vibrato (${stats.waveryTone.wobble} as shipped)`,
);

socket.close();
stop();
