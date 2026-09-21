#!/usr/bin/env node
// Renders the synthesized sounds offline and measures them.
//
// Web Audio only exists in a browser, so this drives a headless one — but it
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
    '--headless', '--no-sandbox', '--disable-gpu',
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
    const { SOUNDS } = await import('./js/sounds.js');
    const RATE = 48000;

    // Renders \`count\` copies of a sound fired at once, through the real chain.
    async function render(name, count, seconds = 1, phoneFilter = false, opts = {}, bank = SOUNDS) {
      const ctx = new OfflineAudioContext(2, RATE * seconds, RATE);
      const audio = new Audio(bank);
      audio.attach(ctx);
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

      let peak = 0, sum = 0, last = 0, crossings = 0, previous = 0;
      for (let i = 0; i < left.length; i += 1) {
        const v = Math.max(Math.abs(left[i]), Math.abs(right[i]));
        if (v > peak) peak = v;
        sum += left[i] * left[i];
        if (v > 0.0008) last = i;
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
        // thirty samples long, so rounding alone adds over a percent of noise —
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
        rms: Number(Math.sqrt(sum / left.length).toFixed(5)),
        ms: Number(((last / RATE) * 1000).toFixed(1)),
        // A rough brightness proxy: noisy, hi-hat-like sounds cross zero often.
        zcrPerSec: Math.round(crossings / (Math.max(1, last) / RATE)),
        wobble,
        early: band(0, third),
        late: band(third * 2, last),
        voices: audio.started,
      };
    }

    // Same sound, no per-play randomness, so the two renders are comparable.
    const steadyBoom = JSON.parse(JSON.stringify(SOUNDS.boom));
    for (const layer of steadyBoom.layers) delete layer.jitter;
    const plain = { boom: steadyBoom };
    const full = await render('boom', 1, 1.2, false, {}, plain);
    const thin = await render('boom', 1, 1.2, true, {}, plain);

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
    const wildTone = await render('tone', 1, 1.5, false, {}, glide({ depth: 0.4, rate: 17 }));
    return {
      one: await render('pop', 1),
      three: await render('pop', 3),
      twenty: await render('pop', 20),
      boom: full,
      boomTwo: await render('boom', 2, 1.2),
      boomFour: await render('boom', 4, 1.2),
      rocketShort: await render('rocket', 1, 2, false, { duration: 0.4 }),
      rocketLong: await render('rocket', 1, 2.5, false, { duration: 1.4 }),
      // How much of the boom survives a speaker that cannot do bass.
      boomThroughPhone: Number((thin.rms / Math.max(1e-9, full.rms)).toFixed(3)),
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
const LIMITER_THRESHOLD = 0.5; // -6 dBFS, where the limiter starts working.
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
  ['glide plain', 'steadyTone'],
  ['glide waver', 'waveryTone'],
  ['glide wild', 'wildTone'],
]) {
  const s = stats[key];
  console.log(
    `  ${label.padEnd(11)} peak ${String(s.peak).padEnd(7)} rms ${String(s.rms).padEnd(8)} ` +
      `tail ${String(s.ms).padStart(6)}ms  bright ${String(s.early).padStart(5)} -> ` +
      `${String(s.late).padStart(5)}  wobble ${String(s.wobble).padEnd(6)} voices ${s.voices}`,
  );
}

const worst = stats.twenty;
if (worst.peak >= LIMITER_THRESHOLD) {
  console.error(
    `\nFAIL: twenty stacked pops peak at ${worst.peak}, at or past the limiter ` +
      `threshold of ${LIMITER_THRESHOLD}. They are meant to stack without it engaging.`,
  );
  stop();
  process.exit(1);
}
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
// already unusual, and four is what the limiter is for — asking a rocket strike
// to be quiet enough that four of them never engage it would only make one of
// them thin. The x4 row is printed to show what the limiter is catching.
const loudest = Math.max(worst.peak, stats.boomTwo.peak);
if (loudest >= LIMITER_THRESHOLD) {
  console.error(
    `\nFAIL: the sounds meant to stack are reaching the limiter — twenty pops peak ` +
      `${worst.peak} and two booms ${stats.boomTwo.peak}, against ${LIMITER_THRESHOLD}.`,
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
// mistake that has already been made here once — 0.16 measured about 0.06 and
// was unmistakable.
//
// The depth itself is set by ear. No number here has an opinion about it.
const WAVER_CEILING = 0.045;
if (stats.wildTone.wobble < stats.steadyTone.wobble * 4) {
  console.error(
    `\nFAIL: the waver does nothing even at an absurd depth — a plain glide wobbles ` +
      `${stats.steadyTone.wobble} and one wavering by 40% only ${stats.wildTone.wobble}. ` +
      `The mechanism is broken.`,
  );
  stop();
  process.exit(1);
}
if (stats.waveryTone.wobble > WAVER_CEILING) {
  console.error(
    `\nFAIL: the shipped waver measures ${stats.waveryTone.wobble}, past ${WAVER_CEILING} — ` +
      `that is a vibrato, not a firework failing to hold its note.`,
  );
  stop();
  process.exit(1);
}

const flightRatio = stats.rocketLong.ms / stats.rocketShort.ms;
if (flightRatio < 2.4 || flightRatio > 4.0) {
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
    `${(stats.boomThroughPhone * 100).toFixed(0)}% of the boom survives a phone speaker; ` +
    `the waver works (${stats.steadyTone.wobble} plain, ${stats.wildTone.wobble} at 40% depth) ` +
    `and is not a vibrato (${stats.waveryTone.wobble} as shipped)`,
);

socket.close();
stop();
