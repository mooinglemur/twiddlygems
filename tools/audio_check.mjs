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
// The profile lives across runs, so without this the sounds measured could be
// the ones cached from a previous run rather than the ones on disk.
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.navigate', { url: `http://127.0.0.1:${PORT}/index.html` });
await sleep(1500);

const { result } = await send('Runtime.evaluate', {
  awaitPromise: true,
  returnByValue: true,
  expression: `
  (async () => {
    const { Audio } = await import('./js/audio.js');
    const { SOUNDS, NOISES, RIFFLE, STEAMBOAT, VICTORY_PARTS } = await import('./js/sounds.js');
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
      // A written figure rather than a single sound: \`name\` may instead be a
      // list of parts, which is what \`Audio.sequence\` takes. Everything
      // measured below is the same either way, which is the point of putting
      // it through the same function.
      // A named filler item rather than a single sound: \`name\` may instead be
      // one of the \`NOISES\` entries, which is a handful of plays or a written
      // figure. Put through the same function as everything else, because a
      // measurement that only ever saw these would have nothing to compare
      // them against.
      if (name && name.plays) {
        for (const play of name.plays) {
          audio.play(play.sound, play);
        }
      } else if (name && name.figure) {
        audio.sequence(name.figure.parts, { tempo: name.figure.tempo, name: 'noise' });
      } else if (Array.isArray(name)) {
        audio.sequence(name, { tempo: STEAMBOAT.tempo, ...opts });
      } else {
        for (let i = 0; i < count; i += 1) {
          audio.play(name, { pan: (i / Math.max(1, count - 1)) * 1.1 - 0.55, ...opts });
        }
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

      // Zero crossings per second over a window, early against late.
      //
      // **This is a brightness proxy for noise and a pitch reading for
      // anything pitched**, and the difference matters. A hiss crosses zero
      // more often the brighter it is, so a filter closing on noise shows up
      // here and that is what separates a poof from a click with a long tail.
      // A harmonic wave crosses zero twice a cycle whatever its harmonics are
      // doing, so on a sawtooth this measures the fundamental: opening a
      // lowpass from 300Hz to 1700Hz over a note moves it by about 2%, while
      // scooping that note up a fifth moves it by 35%. Use \`edge\` below for
      // brightness on anything with a pitch.
      //
      // It is also a rate over the whole window, so silence inside the window
      // dilutes it. The busy signal is two sines that sum to 1240 crossings a
      // second while they sound, and reads 743, because its early third is
      // 832ms containing 500ms of tone: 1240 times 0.6 is 744. Nothing is
      // wrong with the sound. For anything intermittent this figure is the
      // pitch multiplied by the duty cycle.
      const band = (from, to) => {
        let n = 0, prev = 0;
        for (let i = from; i < to; i += 1) {
          const mono = left[i] + right[i];
          if ((mono > 0 && prev <= 0) || (mono < 0 && prev >= 0)) n += 1;
          prev = mono;
        }
        return Math.round(n / (Math.max(1, to - from) / RATE));
      };

      // Brightness that works on a pitched sound: how fast the waveform moves
      // against how big it is.
      //
      // The mean step between samples divided by the mean level. A first
      // difference is a high-pass, so this rises with harmonic content where
      // counting zero crossings does not: a sine's steps are proportional to
      // its frequency, and a sawtooth's are dominated by its highest surviving
      // harmonic, so opening a lowpass over a note shows up directly. Scaled
      // by level so it reads the timbre rather than the envelope.
      //
      // Written for the muted trombone, whose whole character is a filter
      // opening across a held note, and which the crossing count said was
      // doing nothing at all.
      const edge = (from, to) => {
        let steps = 0, size = 0;
        for (let i = Math.max(1, from); i < to; i += 1) {
          const now = left[i] + right[i];
          const was = left[i - 1] + right[i - 1];
          steps += Math.abs(now - was);
          size += Math.abs(now);
        }
        return size > 0 ? Number((steps / size).toFixed(4)) : 0;
      };
      const third = Math.max(1, Math.floor(last / 3));

      // How many separate strikes the sound contains, counted as rising edges
      // of a short-window envelope. One knock or two is audible at a glance but
      // not otherwise checkable, and "two rapid hits" is the whole brief.
      //
      // Only trustworthy well above 300Hz. The window is 3ms, which is shorter
      // than one cycle of anything lower, so a low sine's own waveform shows up
      // as the envelope rising and falling and the count comes out high: the
      // door knock is three strikes and reads as five or six. Fine for the
      // microwave at 2.1kHz, which is the only one asserted on.
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
      // compared against a wide average of its neighbors, so a steady glide
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
        // Each period is compared against the midpoint of two far neighbors
        // rather than an average of everything between. Averaging a window
        // narrower than a few waver steps partly follows the waver and hides
        // it; taking the midpoint of the endpoints cancels the glide's local
        // slope instead, and leaves the wobble behind.
        const REACH = 110;
        // Zero here means "too short to say", not "holds its pitch". It takes
        // a couple of hundred cycles in the last two thirds to measure this at
        // all, which a low sound lasting a third of a second does not have:
        // the boing reads 0 and has one of the deepest wavers in the bank.
        // Anything asserting on this number has to be a sound long enough to
        // produce one.
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

      // How far the pitch moves over the back of the sound, as a fraction of
      // its own period.
      //
      // A different question from \`wobble\`, and it needs a different
      // instrument. \`wobble\` compares each cycle against far neighbors to
      // cancel a glide's slope, and that also cancels a *periodic* swing whose
      // cycles divide into the reach: a 90 cent vibrato at 6Hz on a 220Hz note
      // puts its neighbors exactly three vibrato cycles away, so they sit at
      // the same phase, the midpoint lands on the current value and the whole
      // thing reads 0.014, barely above a dead steady note.
      //
      // This is the plain spread of the instantaneous period, which has
      // nothing to cancel and sees a vibrato for what it is. It sees a glide
      // too, so it only means something between two renders that glide alike.
      const swing = (() => {
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
        if (periods.length < 8) return 0;
        const mean = periods.reduce((sum, p) => sum + p, 0) / periods.length;
        if (mean <= 0) return 0;
        const spread =
          periods.reduce((sum, p) => sum + (p - mean) ** 2, 0) / periods.length;
        return Number((Math.sqrt(spread) / mean).toFixed(4));
      })();

      // How far the sound comes back up after it has fallen.
      //
      // The largest ratio of any later stretch to the quietest stretch before
      // it, taken over block averages after the loudest moment, so the noise
      // in an envelope does not read as a revival. 1 means it only ever gets
      // quieter. Anything well above means the sound dies away and returns,
      // which for a continuous one is a fault and for a deliberately repeating
      // one is the whole point, so what this number means depends on the sound
      // and only the continuous ones are asserted on.
      //
      // Written after the Surf went silent at six seconds and came back at
      // forty percent of its peak at seven, which Troy heard immediately and
      // no check here would have.
      const revive = (() => {
        if (hitEnvelope.length < 16) return 1;
        let loudest = 0;
        for (let i = 0; i < hitEnvelope.length; i += 1) {
          if (hitEnvelope[i] > hitEnvelope[loudest]) loudest = i;
        }
        const tail = hitEnvelope.slice(loudest);
        const size = Math.max(1, Math.floor(tail.length / 16));
        const means = [];
        for (let i = 0; i + size <= tail.length; i += size) {
          let sum = 0;
          for (let j = i; j < i + size; j += 1) sum += tail[j];
          means.push(sum / size);
        }
        if (means.length < 3) return 1;
        // Floored at a hundredth of the peak, which is far below anything
        // audible under a game. Without it a return from true silence divides
        // by nothing and the number is meaningless rather than large.
        let quietest = Math.max(means[0], 0.01);
        let worst = 1;
        for (let i = 1; i < means.length; i += 1) {
          worst = Math.max(worst, means[i] / quietest);
          quietest = Math.max(0.01, Math.min(quietest, means[i]));
        }
        return Number(worst.toFixed(2));
      })();

      return {
        peak: Number(peak.toFixed(4)),
        swing,
        revive,
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
        edgeEarly: edge(0, third),
        edgeLate: edge(third * 2, last),
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
    // A control for \`sweep.by\`, the one new thing the named filler needed: a
    // voice handed its note from a figure and sliding down from wherever it
    // was put, since a fixed destination would slide every note in a figure to
    // the same pitch. The same note both times, so the only difference is the
    // slide, and a slide that silently did nothing would look exactly like a
    // trombone that happened to be playing one note.
    const slide = (by) => ({
      slider: {
        gain: 0.3,
        duration: 0.5,
        layers: [
          {
            source: 'sawtooth',
            ...(by === null ? {} : { sweep: { by, time: 0.42 } }),
            filters: [{ type: 'lowpass', frequency: 1500, q: 1.2 }],
            env: { attack: 0.03, hold: 0.2, decay: 0.3 },
          },
        ],
      },
    });
    const oneNote = [{ sound: 'slider', notes: [['A3', 0, 1]] }];
    const flatNote = await render(oneNote, 1, 2, false, { tempo: 96 }, slide(null));
    const slidNote = await render(oneNote, 1, 2, false, { tempo: 96 }, slide(-700));

    // Controls for the two things the muted trombone needed, measured the same
    // way: one long note on a bare voice, with and without the feature.
    //
    // \`sweep.from\` arrives *on* the written note instead of leaving it, so
    // the early pitch is below the late one, where \`sweep.by\` is the other way
    // round and no sweep at all is flat. Three renders rather than two,
    // because "the pitch rises" means nothing without knowing what the same
    // voice does when it is told to fall.
    const scooper = (sweep) => ({
      scooper: {
        gain: 0.3,
        duration: 1,
        layers: [
          {
            source: 'sawtooth',
            ...(sweep ? { sweep } : {}),
            filters: [{ type: 'lowpass', frequency: 1400, q: 1 }],
            env: { attack: 0.02, hold: 0.7, decay: 0.2 },
          },
        ],
      },
    });
    const held = [{ sound: 'scooper', notes: [['A3', 0, 1]] }];
    const scoopFlat = await render(held, 1, 3, false, { tempo: 60 }, scooper(null));
    const scoopUp = await render(held, 1, 3, false, { tempo: 60 }, scooper({ from: -700, time: 0.5 }));
    const scoopDown = await render(held, 1, 3, false, { tempo: 60 }, scooper({ by: -700, time: 0.5 }));

    // And \`vibrato\`, which starts partway through and runs to the end. The
    // wobble figure is read over the last two thirds, which is where the
    // vibrato is and where the control is dead steady, so on this voice the
    // number is the vibrato and nothing else. A real periodic swing, unlike
    // the \`waver\` random walk, and deep enough here to be unmistakable.
    const vibrato = (on) => ({
      holder: {
        gain: 0.3,
        duration: 1,
        layers: [
          {
            source: 'sawtooth',
            ...(on ? { vibrato: { depth: 90, rate: 6, after: 0.4 } } : {}),
            filters: [{ type: 'lowpass', frequency: 1400, q: 1 }],
            env: { attack: 0.02, hold: 1.4, decay: 0.3 },
          },
        ],
      },
    });
    const steadyHeld = [{ sound: 'holder', notes: [['A3', 0, 1]] }];
    const noVibrato = await render(steadyHeld, 1, 3, false, { tempo: 60 }, vibrato(false));
    const withVibrato = await render(steadyHeld, 1, 3, false, { tempo: 60 }, vibrato(true));

    // The shipped trombone on one long note, so the plunger opening can be
    // seen. Within the figure it cannot: the brightness figures are thirds of
    // the whole four-note phrase, and the wah happens inside each note.
    const wahNote = await render([{ sound: 'brass', notes: [['A3', 0, 1]] }], 1, 3, false, {
      tempo: 60,
    });

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
      thudThree: await worst('thud', 3, 0.6),
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
      bell: await render('bell', 1, 2),
      // At 300ms apart and over a second of ring, this many overlap during
      // the run down at the end of a level.
      bellFour: await worst('bell', 4, 2),
      chimeFirst: await render('chime', 1, 0.8, false, { stage: 0 }),
      chimeLast: await render('chime', 1, 0.8, false, { stage: 11 }),
      // Past the end of the progression it should hold, not wrap round.
      chimePastEnd: await render('chime', 1, 0.8, false, { stage: 40 }),
      chimeStages: SOUNDS.chime.chords.length,
      // Measured but not judged. This one is a placeholder for Troy's ear;
      // what the number is for is so he can see what it costs while he tunes
      // it, not so a check can have an opinion about how it should sound.
      fanfare: await render('fanfare', 1, 2.6),
      // The victory tune. Measured but, like the fanfare, not judged on how it
      // sounds: that is Troy's ear. What is judged is what a measurement can
      // actually settle, which is whether three voices playing at once for
      // twenty seconds stay under the limiter, and whether \`hush\` works.
      tune: await render(VICTORY_PARTS, 1, 26),
      // Each voice alone, over the whole tune rather than as one note.
      //
      // One note would be quicker and would measure nothing: a voice's peak is
      // a random variable for the same reason a stack's is, since the two
      // detuned oscillators in \`keys\` are phase-scattered per note and how
      // squarely they line up decides it. Read as one note the melody came out
      // at less than half the level of its own accompaniment, which was the
      // measurement talking. Across eighty-nine notes the \`rmsBuffer\` figure is
      // steady, and it is what the parts are balanced against.
      keys: await render([VICTORY_PARTS[0]], 1, 26),
      glass: await render([VICTORY_PARTS[1]], 1, 26),
      bass: await render([VICTORY_PARTS[2]], 1, 26),
      tuneNotes: VICTORY_PARTS.reduce((n, part) => n + part.notes.length, 0),
      tuneSeconds: Number(
        (
          (Math.max(
            ...VICTORY_PARTS.flatMap((part) => part.notes.map(([, beat, held]) => beat + held)),
          ) *
            60) /
          STEAMBOAT.tempo
        ).toFixed(1),
      ),
      /**
       * What is left of the tune a second after it was hushed.
       *
       * The one thing about a figure that a measurement can be certain of and
       * a person cannot check by listening carefully: \`hush\` is what stops
       * the tune playing on over whatever the player went to next, and a
       * sequence is scheduled all at once, so nothing else in the graph will
       * stop it. Against a control that was not hushed, so the number is a
       * ratio and not a level that moves whenever a gain does.
       */
      hushed: await (async () => {
        const window = (buffer, from) => {
          const left = buffer.getChannelData(0);
          const right = buffer.getChannelData(1);
          let sum = 0;
          for (let i = Math.floor(from * RATE); i < left.length; i += 1) {
            sum += left[i] * left[i] + right[i] * right[i];
          }
          return Math.sqrt(sum / Math.max(1, left.length - Math.floor(from * RATE)));
        };
        const run = async (stop) => {
          const ctx = new OfflineAudioContext(2, RATE * 6, RATE);
          const audio = new Audio(SOUNDS);
          audio.attach(ctx);
          audio.master.disconnect();
          audio.master.connect(ctx.destination);
          audio.sequence(VICTORY_PARTS, { tempo: STEAMBOAT.tempo, name: 'victory' });
          if (stop) {
            audio.hush('victory', 0.25);
          }
          return window(await ctx.startRendering(), 1);
        };
        const playing = await run(false);
        const stopped = await run(true);
        return {
          playing: Number(playing.toFixed(6)),
          stopped: Number(stopped.toFixed(6)),
          ratio: Number((stopped / Math.max(1e-9, playing)).toFixed(5)),
        };
      })(),
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
      flatNote,
      slidNote,
      scoopFlat,
      scoopUp,
      scoopDown,
      noVibrato,
      withVibrato,
      wahNote,
      /**
       * Wavering layers whose glide ends well before the layer does.
       *
       * Read off the definitions, not out of a render: this is arithmetic on
       * the numbers that make the sound, so it is exact where a measurement of
       * it is not. See the check that reads this.
       *
       * A third is the line. Every wavering layer that ships settles for the
       * last 3% to 16% of itself, during the decay, which is both deliberate
       * and inaudible; the fault this is here for sat still for 57%.
       */
      stillWavers: (() => {
        const found = [];
        for (const [name, sound] of Object.entries(SOUNDS)) {
          const layers = [...(sound.layers ?? []), ...(sound.voice ? [sound.voice] : [])];
          layers.forEach((layer, at) => {
            if (!layer.waver) return;
            const e = layer.env ?? {};
            // The same sum \`playLayer\` makes, and the same defaults.
            const env = (e.attack ?? 0.002) + (e.hold ?? 0) + (e.decay ?? 0.1);
            const glide = layer.sweep?.time ?? env;
            const still = env - glide;
            if (still / env > 1 / 3) {
              found.push({
                sound: name,
                layer: at,
                glide: Number(glide.toFixed(3)),
                env: Number(env.toFixed(3)),
                still: Number(still.toFixed(3)),
                fraction: Number((still / env).toFixed(3)),
              });
            }
          });
        }
        return found;
      })(),
      /**
       * Every named filler sound, measured the way the page plays it.
       *
       * Twenty-six of these and nobody is going to listen to all of them on
       * every build, so what is checked here is what a measurement can settle
       * and an ear cannot be relied on to: that each one makes a sound at all,
       * and that none of them is loud enough to meet the limiter on its own.
       * A sound whose name was mistyped, or whose layers all cancel, is
       * silence, and silence is the one failure that looks exactly like an
       * item that does nothing on purpose.
       *
       * Rendered one at a time rather than together, because what each one
       * costs on its own is the question. How they stack is not a question
       * here: these arrive one per check, and the two paths that play them
       * both go through one item.
       */
      noises: await (async () => {
        // The whole bank with the per-play randomness taken out, for the phone
        // comparison below only.
        //
        // The pair has to be two renders of the *same* sound or the ratio
        // measures the dice instead of the filter. Measured against two live
        // renders, the door knock came out at 54% one run and 36% the next,
        // which is the jitter on its pitch moving a 96Hz fundamental around a
        // 200Hz cutoff, not anything about a phone. Same trap the boom and the
        // thud figures already avoid, a few hundred lines up.
        const steady = JSON.parse(JSON.stringify(SOUNDS));
        for (const sound of Object.values(steady)) {
          delete sound.scatter;
          for (const layer of [...(sound.layers ?? []), ...(sound.voice ? [sound.voice] : [])]) {
            delete layer.jitter;
          }
        }
        // Long enough for the longest of them with room to spare. The surf
        // has a 15.3s envelope, and a window shorter than the sound measures
        // the window: the tail reads as exactly the buffer length and the
        // average is taken over a sound that was cut off, which makes a long
        // quiet one look like a short loud one. It also hides a revival that
        // happens past the end of the window, which is the one thing the
        // \`revive\` figure is here to catch.
        const WINDOW = 17;
        const out = [];
        for (const noise of NOISES) {
          // What ships, played the way the page plays it, and the loudest of
          // several goes.
          //
          // A peak here is a random variable like a stack's is, for the same
          // two reasons: a few of these are several plays at once, and every
          // oscillator starts on a scattered phase. Measured once, the wind
          // chime came back anywhere between 0.33 and 0.55 across runs, so a
          // threshold read against one draw is a threshold that fires on a
          // Tuesday. What the limiter check below wants to know is whether a
          // sound *can* clip, so the worst of a handful is the honest answer.
          const full = await worst(noise, 1, WINDOW, 4);
          // And the steady pair, whole and then through what a phone speaker
          // throws away. Worth knowing per sound rather than in general: the
          // bank runs from a door knock with a 96Hz fundamental to a cricket
          // at 4.4kHz, so how much of one survives a small speaker is a
          // property of that one sound. Printed and not judged, because a
          // breaking wave is allowed to be mostly bass; what it is for is so
          // Troy can see what a sound costs on a phone while he is deciding
          // whether he likes it on a desk.
          const wide = await render(noise, 1, WINDOW, false, {}, steady);
          const thin = await render(noise, 1, WINDOW, true, {}, steady);
          out.push({
            name: noise.name,
            ...full,
            through: Number((thin.rmsBuffer / Math.max(1e-9, wide.rmsBuffer)).toFixed(3)),
          });
        }
        return out;
      })(),
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
  ['bell', 'bell'],
  ['bell x4', 'bellFour'],
  ['chime 1/12', 'chimeFirst'],
  ['chime 12/12', 'chimeLast'],
  ['fanfare', 'fanfare'],
  ['keys', 'keys'],
  ['glass', 'glass'],
  ['bass', 'bass'],
  ['tune', 'tune'],
  ['glide plain', 'steadyTone'],
  ['glide waver', 'waveryTone'],
  ['glide wild', 'wildTone'],
  ['slide none', 'flatNote'],
  ['slide -700c', 'slidNote'],
  ['held plain', 'scoopFlat'],
  ['scoop up', 'scoopUp'],
  ['slide off', 'scoopDown'],
  ['held steady', 'noVibrato'],
  ['held vibrato', 'withVibrato'],
  ['brass 1 note', 'wahNote'],
]) {
  const s = stats[key];
  console.log(
    `  ${label.padEnd(11)} peak ${String(s.peak).padEnd(7)} rms ${String(s.rms).padEnd(8)} ` +
      `from ${String(s.onsetMs).padStart(5)}ms  tail ${String(s.ms).padStart(6)}ms  ` +
      `bright ${String(s.early).padStart(5)} -> ` +
      `${String(s.late).padStart(5)}  edge ${String(s.edgeEarly).padStart(6)} -> ` +
      `${String(s.edgeLate).padStart(6)}  swing ${String(s.swing).padEnd(6)}`,
  );
}

console.log('\nthe named filler, one at a time:');
for (const s of stats.noises) {
  console.log(
    `  ${s.name.padEnd(20)} peak ${String(s.peak).padEnd(7)} rms ${String(s.rms).padEnd(8)} ` +
      `from ${String(s.onsetMs).padStart(5)}ms  tail ${String(s.ms).padStart(6)}ms  ` +
      `bright ${String(s.early).padStart(5)} -> ${String(s.late).padStart(5)}  ` +
      `hits ${String(s.hits).padStart(2)}  ` +
      `back ${String(s.revive).padStart(5)}  ` +
      `phone ${String(Math.round(s.through * 100)).padStart(3)}%`,
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
// The thud is asserted at three, not at a landing per column.
//
// A whole board landing at once needs every column to be dropping the same
// distance at the same moment, which wants a full-width clear and essentially
// never happens; landings are grouped by column and by depth, so a big cascade
// spreads them out rather than stacking them. Ordinary play is the three or
// four that `make balance` measures, and the rare board-wide case is what the
// limiter is for. Its row is printed.
const loudest = Math.max(
  stats.three.peak,
  stats.boomTwo.peak,
  stats.sparkleTwenty.peak,
  stats.thudThree.peak,
);
if (loudest >= LIMITER_THRESHOLD) {
  console.error(
    `\nFAIL: ordinary play is reaching the limiter. Three pops peak ${stats.three.peak}, ` +
      `twenty sparkles ${stats.sparkleTwenty.peak}, two booms ${stats.boomTwo.peak} and ` +
      `three thuds ${stats.thudThree.peak}, against ${LIMITER_THRESHOLD}.`,
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
// The low-moves bell is a figure rather than a single beep, which is what makes
// it read as a chime and not an alarm.
//
// How many notes, and whether they rise or fall, is not asserted. It started
// here as two notes falling and is now three that climb, because it is set by
// ear and the ear changed its mind. A check that pinned the shape would only
// have to be argued with every time.
if (stats.ding.hits < 2) {
  console.error(
    `\nFAIL: the low-moves bell is a single strike; it is meant to be a figure.\n` +
      `  envelope (3ms per step, relative): ${stats.ding.envelope.join(' ')}`,
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
// Three voices playing at once for twenty seconds is the one thing in the game
// that sustains, so unlike a cascade it is not the limiter's to catch: it would
// be pumping the whole tune rather than meeting one transient.
if (stats.tune.peak > LIMITER_THRESHOLD) {
  console.error(
    `\nFAIL: the victory tune peaks ${stats.tune.peak}, over the limiter at ` +
      `${LIMITER_THRESHOLD}. A sustained figure should not be leaning on it. ` +
      `Voices alone: keys ${stats.keys.peak}, glass ${stats.glass.peak}, bass ${stats.bass.peak}.`,
  );
  stop();
  process.exit(1);
}
// And it must be audible, because a tune mixed into nothing is the same bug as
// one that never played and is much harder to notice.
if (stats.tune.peak < 0.08) {
  console.error(`\nFAIL: the victory tune peaks ${stats.tune.peak}, which is barely there.`);
  stop();
  process.exit(1);
}
// The figure has to actually last: a sequencer that scheduled every note at
// beat zero would still play, still measure loud, and be over in a second.
if (stats.tune.ms < stats.tuneSeconds * 900) {
  console.error(
    `\nFAIL: the tune is written to run ${stats.tuneSeconds}s but sounded for ` +
      `${(stats.tune.ms / 1000).toFixed(1)}s. The beats are not being spread out.`,
  );
  stop();
  process.exit(1);
}
// The melody has to be the loudest thing in the arrangement. Obvious, easy to
// lose while tuning a gain somewhere, and inaudible as a fault until someone
// notices the tune sounds oddly bottom-heavy without being able to say why.
//
// By a margin rather than by any amount. A voice's level still varies a little
// between renders, because each note's oscillators start on a random phase, so
// "ahead by a hair on this run" is not a property of the arrangement. The
// margin is what makes the check about the mix instead of about the render.
const MELODY_MARGIN = 1.15;
const accompaniment = Math.max(stats.glass.rmsBuffer, stats.bass.rmsBuffer);
if (stats.keys.rmsBuffer < accompaniment * MELODY_MARGIN) {
  console.error(
    `\nFAIL: the melody does not lead the victory tune by the ${MELODY_MARGIN}x asked for. ` +
      `keys ${stats.keys.rmsBuffer}, glass ${stats.glass.rmsBuffer}, bass ${stats.bass.rmsBuffer}.`,
  );
  stop();
  process.exit(1);
}
// `hush` is what stops it playing on over whatever the player went to next.
//
// The control first. The check below is a ratio, so two silent renders would
// divide to zero and pass it while proving nothing at all: what makes the
// ratio mean something is that the tune really was sounding a second in when
// it was left alone.
if (stats.hushed.playing < 0.005) {
  console.error(
    `\nFAIL: the un-hushed control is silent a second in (${stats.hushed.playing}), so the ` +
      `hush check below would pass whatever hush did.`,
  );
  stop();
  process.exit(1);
}
if (stats.hushed.ratio > 0.02) {
  console.error(
    `\nFAIL: a second after being hushed the tune is still at ` +
      `${(stats.hushed.ratio * 100).toFixed(1)}% of its level. It has to stop.`,
  );
  stop();
  process.exit(1);
}

// `sweep.by` is the one piece of new machinery the named filler needed, and a
// slide that quietly did nothing would leave the trombone sounding like an
// organ: still a sound, still the right notes, nothing obviously broken. The
// same note with and without a seven-semitone slide, so the only thing that
// can move the late brightness is the slide.
if (stats.slidNote.late >= stats.flatNote.late * 0.9) {
  console.error(
    `\nFAIL: a note told to slide down 700 cents ends at ${stats.slidNote.late} against the ` +
      `${stats.flatNote.late} of the same note not sliding. \`sweep.by\` is not gliding the ` +
      `pitch, so every voice handed its note from a figure is holding it flat.`,
  );
  stop();
  process.exit(1);
}

// Every named filler sound has to be a sound.
//
// Silence is the failure that hides: an item whose sound name is mistyped, or
// whose layers cancel, arrives with its name in the feed and does nothing,
// which is exactly what an item that does nothing on purpose looks like.
const silent = stats.noises.filter((noise) => noise.peak < 0.002);
if (silent.length > 0) {
  console.error(
    `\nFAIL: ${silent.length} of the ${stats.noises.length} named filler sounds are silent: ` +
      `${silent.map((noise) => `${noise.name} (${noise.peak})`).join(', ')}.`,
  );
  stop();
  process.exit(1);
}
// And none of them may meet the limiter on its own. These arrive one at a
// time, so unlike a pile of pops there is nothing here for the limiter to be
// catching: one of these over the threshold is simply too loud.
const tooLoud = stats.noises.filter((noise) => noise.peak >= LIMITER_THRESHOLD);
if (tooLoud.length > 0) {
  console.error(
    `\nFAIL: ${tooLoud.length} named filler sounds reach the limiter on their own, against ` +
      `${LIMITER_THRESHOLD}: ${tooLoud.map((n) => `${n.name} (${n.peak})`).join(', ')}.`,
  );
  stop();
  process.exit(1);
}
// They also have to be in the same ballpark as each other.
//
// A sound at a twentieth of the level of its neighbors passes the silence
// check above and is still useless: the item arrives, something happens, and
// the player cannot tell what. Measured as a ratio against the middle of the
// set rather than against a fixed level, because what is wanted is that they
// sit together, not that they sit at any particular place. Three of these came
// in at a quarter of the median the first time they were measured, which is
// how this check came to exist.
//
// Averaged over each sound's own extent, not peaked: a struck note peaks many
// times its average while dense noise barely peaks at all, so peaks would
// compare the shapes of these sounds rather than their loudness.
const levels = stats.noises.map((noise) => noise.rms).sort((a, b) => a - b);
const median = levels[Math.floor(levels.length / 2)];
const faint = stats.noises.filter((noise) => noise.rms < median / 8);
if (faint.length > 0) {
  console.error(
    `\nFAIL: ${faint.length} named filler sounds are under an eighth of the median level ` +
      `(${median.toFixed(5)}): ${faint.map((n) => `${n.name} (${n.rms})`).join(', ')}. ` +
      `They will not be heard as the same kind of event as the rest.`,
  );
  stop();
  process.exit(1);
}

// `sweep.from` has to arrive on the note rather than leave it.
//
// Both directions are checked against the same voice holding still, because
// "the pitch rose" is only meaningful next to what the voice does when told to
// fall. A scoop whose sign was inverted would still be a glide, still sound
// like a trombone of a sort, and be the wrong gesture: Troy asked for a scoop
// up into the note having been given a slide off it, so this is the mistake
// the feature exists to stop repeating.
// The crossing count is the right ruler here and the wrong one for the wah
// below: on a pitched sound it reads the fundamental, which is exactly what a
// scoop moves. The arrival is compared with a few percent of slack, because
// counting crossings in a window is a couple of counts noisy and "arrives on
// pitch" is not a claim about the third decimal.
if (
  !(
    stats.scoopUp.early < stats.scoopFlat.early * 0.9 &&
    stats.scoopUp.late > stats.scoopFlat.late * 0.95
  )
) {
  console.error(
    `\nFAIL: a note told to scoop up from 700 cents below reads ${stats.scoopUp.early} -> ` +
      `${stats.scoopUp.late} against ${stats.scoopFlat.early} -> ${stats.scoopFlat.late} for ` +
      `the same note held flat. It should start below and arrive on pitch.`,
  );
  stop();
  process.exit(1);
}
if (stats.scoopDown.late >= stats.scoopFlat.late) {
  console.error(
    `\nFAIL: \`sweep.by\` and \`sweep.from\` are not opposites: sliding off a note ends at ` +
      `${stats.scoopDown.late} against ${stats.scoopFlat.late} held flat, so it is not ` +
      `falling. One of the two directions is wired wrong.`,
  );
  stop();
  process.exit(1);
}

// `vibrato` has to actually swing, and the control has to actually be steady.
//
// Read with `swing` and not `wobble`: see the note where both are computed.
// `wobble` cancels a periodic modulation and scored this 0.014 against 0.005
// for a dead steady note, which looks like a vibrato that is barely working
// and is really a ruler that cannot see one.
//
// The control first, for the same reason the hush check has one: if a held
// note already moved about, this would pass on a vibrato that did nothing.
// Compared as a ratio against the same voice, not against an absolute, because
// this number has a noise floor that depends on the sound: interpolating zero
// crossings of a filtered sawtooth is a few parts in a thousand jittery by
// itself, and a shorter note has fewer cycles to average over. Two renders of
// one voice share that floor, so the ratio is the honest figure.
if (stats.noVibrato.swing > 0.02) {
  console.error(
    `\nFAIL: a held note with no vibrato already moves ${stats.noVibrato.swing}, which is too ` +
      `much to tell a vibrato from, so the check below would prove nothing.`,
  );
  stop();
  process.exit(1);
}
// Both an absolute floor and a ratio. The ratio alone has a hole: the control
// legitimately measures 0 on a lucky phase, and `0 < 0 * 2` is false, so a
// vibrato that did nothing at all would pass. 0.018 is half of what 90 cents
// should give.
if (stats.withVibrato.swing < Math.max(0.018, stats.noVibrato.swing * 2)) {
  console.error(
    `\nFAIL: a 90 cent vibrato spreads the pitch ${stats.withVibrato.swing} against ` +
      `${stats.noVibrato.swing} for the same note without one, where 90 cents should be ` +
      `about 0.037. It is not swinging.`,
  );
  stop();
  process.exit(1);
}

// And the plunger has to be doing the wah, not the pitch.
//
// The shipped voice on one long note, rather than a synthetic control, because
// the thing worth pinning is the real trombone. Troy's correction was that the
// wah had been a pitch glide down, so every note sagged off its own pitch;
// what a mute does is open the bell, which changes the timbre and not the
// pitch at all.
//
// Read with `edge` and not with the crossing count. The crossing count says
// this voice goes 432 -> 443 across a note whose filter opens from 300Hz to
// 1700Hz, because a sawtooth crosses zero twice a cycle however many harmonics
// are getting out: it was measuring the pitch, which correctly barely moves.
if (stats.wahNote.edgeLate < stats.wahNote.edgeEarly * 1.35) {
  console.error(
    `\nFAIL: the muted trombone's timbre goes ${stats.wahNote.edgeEarly} -> ` +
      `${stats.wahNote.edgeLate} across one long note, so it is not opening up. The plunger ` +
      `coming off the bell is the whole of the "wah", and it is a filter sweep rather than ` +
      `anything to do with pitch.`,
  );
  stop();
  process.exit(1);
}

// A sound that is one continuous event must only ever get quieter.
//
// Named rather than applied to all twenty, because most of the rest come back
// on purpose: the Kitchen Timer and the Cricket Chirp are three bursts each,
// the Wind Chime is four separate tubes, the Door Knock is three knocks. For
// those, dying away and returning is the design. For a swell of surf it is a
// fault, and it was one: the tail arrived as a third layer fading in after the
// first two had gone, so the sound stopped at six seconds and restarted at
// forty percent of its peak.
//
// Missing is a failure, the same as the Kitchen Timer's check, because a
// rename would otherwise take the check with it silently.
for (const name of ['Surf', 'Gust of Wind']) {
  const sound = stats.noises.find((noise) => noise.name === name);
  if (!sound) {
    console.error(
      `\nFAIL: there is no ${name} among the named filler, so the check that it fades away ` +
        `once rather than twice is checking nothing. Point it at whatever replaced it.`,
    );
    stop();
    process.exit(1);
  }
  // Two is generous. A monotonic decay reads 1, and the fault that prompted
  // this read in the tens.
  if (sound.revive > 2) {
    console.error(
      `\nFAIL: ${name} comes back to ${sound.revive} times its quietest moment after having ` +
        `faded. It is one continuous sound, so it should only ever get quieter: something in ` +
        `it is swelling again after the rest has gone.`,
    );
    stop();
    process.exit(1);
  }
}

// A layer that wavers must waver for most of its own length.
//
// Troy heard this one before any check did. The Barking Spider was lengthened
// to 1.4s with its glide left at the 0.6s it had when the sound was 0.7s, so
// the last 0.8s sat dead still: "incorrectly stops changing pitch at some
// point." The waver stops when the glide does, because the waver *is* steps
// along the glide and there are none once it has arrived, so what went flat was
// the wobble and not only the climb.
//
// Read off the definitions rather than out of a render, which is the second
// attempt at this. The first compared the measured `wobble` against the
// rocket's and did not discriminate at all: the broken version scored 0.51 and
// the fixed one 0.20, because that metric compares each cycle against
// neighbors a couple of hundred periods away, and where the glide stops the
// reach straddles the moving part and the still part and reports the boundary
// as enormous deviation. The audio cannot settle this; the numbers that make
// the sound can, exactly.
//
// A plain glide that arrives and holds is ordinary, which is why only wavered
// layers are checked: the thud's pitch drops in 55ms and then the body rings
// for twice that, and it is right. A `waver` exists to stop a pitch settling,
// so a wavered layer that settles is contradicting itself.
if (stats.stillWavers.length > 0) {
  console.error(
    `\nFAIL: ${stats.stillWavers.length} wavering layers hold a dead pitch for most of ` +
      `their length:\n` +
      stats.stillWavers
        .map(
          (l) =>
            `  ${l.sound} layer ${l.layer}: glides for ${l.glide}s of ${l.env}s, ` +
            `then still for ${l.still}s (${Math.round(l.fraction * 100)}%)`,
        )
        .join('\n') +
      `\nA waver is steps along the glide, so once the glide arrives the pitch stops moving ` +
      `entirely. Leave \`time\` off the sweep to glide across the whole envelope.`,
  );
  stop();
  process.exit(1);
}

// One of them is twelve beeps and nothing else, which is the one claim in the
// whole set a measurement can check as squarely as an ear: four to a burst,
// three bursts, evenly spaced and all the same level. It is also the only
// sound here whose pitch is well clear of the strike counter's 3ms window, so
// if the counter and this sound ever disagree, one of the two is wrong.
//
// Looked up by name, and **missing is a failure**. This check was written for
// Microwave Beep; renaming that item to Kitchen Timer left the lookup finding
// nothing and the check quietly passing on every build, which is worse than
// not having it. Anything found by name in here has to say so when it is gone.
const BURSTS = 3;
const PER_BURST = 4;
const beeps = stats.noises.find((noise) => noise.name === 'Kitchen Timer');
if (!beeps) {
  console.error(
    `\nFAIL: there is no Kitchen Timer among the named filler, so the one sound whose shape ` +
      `this can check exactly is not being checked. Point this at whatever replaced it.`,
  );
  stop();
  process.exit(1);
}
if (beeps.hits !== BURSTS * PER_BURST) {
  // The envelope is one value every 3ms over a thirteen second window, so it
  // is four thousand numbers and all but a tenth of them are zero. Shown as
  // the loud windows only, which is what the count is made of and short
  // enough to read.
  const shape = beeps.envelope
    .map((level, at) => (level > 0.35 ? at : -1))
    .filter((at) => at >= 0)
    .map((at) => `${(at * 3) / 1000}s`);
  console.error(
    `\nFAIL: the Kitchen Timer beeps ${beeps.hits} times rather than ` +
      `${BURSTS * PER_BURST}, which is ${PER_BURST} to a burst and ${BURSTS} bursts. ` +
      `It is loud at ${shape.length} points: ${shape.slice(0, 40).join(' ')}` +
      `${shape.length > 40 ? ' ...' : ''}`,
  );
  stop();
  process.exit(1);
}

const loudestNoise = stats.noises.reduce((worstOne, noise) =>
  noise.peak > worstOne.peak ? noise : worstOne,
);
// Called out rather than failed on. Half of one of these surviving a phone is
// not a fault, it is what a low sound is; what would be a fault is not knowing
// which ones they are while tuning them.
const bassy = stats.noises.filter((noise) => noise.through < 0.5);
console.log(
  `\nnamed filler ok: ${stats.noises.length} sounds, all audible and all under the limiter; ` +
    `loudest is ${loudestNoise.name} at ${loudestNoise.peak}; ` +
    `a 700 cent slide drops the late pitch ${stats.flatNote.late} -> ${stats.slidNote.late}`,
);
if (bassy.length > 0) {
  console.log(
    `  under half of these reaches a phone speaker: ` +
      `${bassy.map((n) => `${n.name} ${Math.round(n.through * 100)}%`).join(', ')}`,
  );
}

console.log(
  `\naudio ok: loudest stack peaks ${loudest.toFixed(3)} (limiter at ${LIMITER_THRESHOLD}); ` +
    `the pop darkens ${stats.one.early} -> ${stats.one.late}; ` +
    `${(stats.boomThroughPhone * 100).toFixed(0)}% of the boom and ` +
    `${(stats.thudThroughPhone * 100).toFixed(0)}% of the thud survive a phone speaker; ` +
    `the waver works (${stats.steadyTone.wobble} plain, ${stats.wildTone.wobble} at 40% depth) ` +
    `and is not a vibrato (${stats.waveryTone.wobble} as shipped); ` +
    `the victory tune runs ${stats.tuneSeconds}s of ${stats.tuneNotes} notes across three ` +
    `voices, peaking ${stats.tune.peak}, and hushes to ` +
    `${(stats.hushed.ratio * 100).toFixed(2)}%`,
);

socket.close();
stop();
