// Web Audio playback for the sounds defined in sounds.js.
//
// Everything is synthesized: no files to download, no format to negotiate, and
// a sound can be pitched per play rather than shipped as variants.
//
// Two things shape the design. Browsers will not start audio until the player
// has touched the page, so the context is created on the first gesture and
// anything asked for before that is dropped. And clears arrive from the engine
// with a delay attached, so sounds are *scheduled* on the audio clock rather
// than fired from a timer: a blast sweeping along a row keeps its rhythm even
// if the frame loop stutters.

import { SOUNDS } from './sounds.js';

const A4 = 440;
const NOTE_OFFSETS = { C: -9, D: -7, E: -5, F: -4, G: -2, A: 0, B: 2 };

/// Converts 'C#5' or 'Bb3' to hertz. Numbers pass through as hertz already.
export function noteToHz(note) {
  if (typeof note === 'number') {
    return note;
  }
  const match = /^([A-Ga-g])([#b]?)(-?\d+)$/.exec(String(note).trim());
  if (!match) {
    return A4;
  }
  const [, letter, accidental, octave] = match;
  let semitones = NOTE_OFFSETS[letter.toUpperCase()];
  if (accidental === '#') semitones += 1;
  if (accidental === 'b') semitones -= 1;
  semitones += (Number(octave) - 4) * 12;
  return A4 * 2 ** (semitones / 12);
}

/// How many voices of one sound may overlap. A rainbow taking a whole color
/// can ask for twenty at once; past this they are simply dropped, because the
/// twentieth copy of a noise burst is inaudible under the other nineteen.
const DEFAULT_VOICE_CAP = 10;

export class Audio {
  constructor(sounds = SOUNDS) {
    this.sounds = sounds;
    this.ctx = null;
    this.master = null;
    this.noise = null;
    this.enabled = true;
    /// What is still sounding, by name: one entry per voice, saying when it
    /// ends on the audio clock and holding the node to unplug once it has.
    ///
    /// The audio clock and not a timer. A voice used to be released by
    /// `setTimeout`, which a phone under load delays and a backgrounded tab
    /// throttles hard. Delay those and the count sticks at the cap, every
    /// later sound of that name is dropped on the floor, and the game goes
    /// quiet or lets a few through in pieces. The clock that decides when a
    /// sound actually ends is the one that should say when its voice is free,
    /// and it cannot run late because it is the same clock the sound is
    /// scheduled on.
    this.voices = new Map();
    this.started = 0;
  }

  get ready() {
    // An OfflineAudioContext is never "running" (it reports suspended until it
    // renders), so it is recognized by the method only it has. Tests render
    // the same graph the game plays.
    return (
      this.ctx !== null &&
      (this.ctx.state === 'running' || typeof this.ctx.startRendering === 'function')
    );
  }

  /// Opens the audio device. Must be called from a user gesture, which is what
  /// every browser requires and what iOS is strictest about.
  unlock() {
    if (this.ctx) {
      if (this.ctx.state === 'suspended') {
        this.ctx.resume().catch(() => {});
      }
      return;
    }
    const Ctor = window.AudioContext || window.webkitAudioContext;
    if (!Ctor) {
      this.enabled = false;
      return;
    }
    try {
      this.attach(new Ctor());
    } catch (error) {
      console.warn('no audio available', error);
      this.enabled = false;
    }
  }

  /// Builds the output chain on a context. Separate from `unlock` so a test can
  /// render the same graph through an OfflineAudioContext.
  attach(ctx) {
    this.ctx = ctx;

    // A limiter on the end, as a safety net rather than a mixing tool.
    //
    // It sits just under the ceiling, not down in the mix. At -6dB it was
    // catching an ordinary three-gem clear, which peaks around -4dB before it
    // gets here, so it was shaping every cascade rather than saving the rare
    // one, and the levels were being tuned by ear through a compressor that was
    // quietly doing the work. Up here it only meets genuine overs.
    const limiter = this.ctx.createDynamicsCompressor();
    limiter.threshold.value = -1.5;
    limiter.knee.value = 0;
    limiter.ratio.value = 20;
    limiter.attack.value = 0.003;
    limiter.release.value = 0.25;
    // Kept to hand so a test can read the threshold it is checking against
    // rather than carrying its own copy of the number.
    this.limiter = limiter;

    this.master = this.ctx.createGain();
    // Honors the setting rather than assuming sound is wanted. The context is
    // opened on the first gesture, which is long after the saved preference
    // was applied to a player who had none yet.
    this.master.gain.value = this.enabled ? 0.9 : 0;
    this.master.connect(limiter);
    limiter.connect(this.ctx.destination);

    this.noise = makeNoiseBuffer(this.ctx);
    if (this.ctx.state === 'suspended' && this.ctx.resume) {
      this.ctx.resume().catch(() => {});
    }
  }

  setEnabled(on) {
    this.enabled = on;
    if (this.master) {
      this.master.gain.value = on ? 0.9 : 0;
    }
  }

  /// Plays a named sound. `delay` is in seconds from now, and is scheduled on
  /// the audio clock rather than with a timer.
  play(
    name,
    { delay = 0, pan = 0, gain = 1, detune = 0, note = null, duration = null, stage = 0 } = {},
  ) {
    if (!this.enabled || !this.ready) {
      return false;
    }
    const sound = this.sounds[name];
    if (!sound) {
      return false;
    }

    // Before the cap is read, so what it is read against is what is actually
    // still sounding rather than everything ever started.
    this.sweep();
    const cap = sound.voiceCap ?? DEFAULT_VOICE_CAP;
    const live = this.voices.get(name) ?? [];
    if (live.length >= cap) {
      return false;
    }
    this.started += 1;

    // `scatter` holds a sound back by a random moment of its own. Twenty gems
    // cleared together ask for twenty of these at the same instant; without it
    // they arrive as one event rather than as a shimmer spreading out.
    const scatter = Math.random() * Math.max(0, sound.scatter ?? 0);
    const at = this.ctx.currentTime + Math.max(0, delay) + scatter;
    const level = (sound.gain ?? 1) * gain;
    // A sound with a declared length can be asked to fill a different one. A
    // rocket's whistle has to last exactly as long as its flight, and flights
    // vary with distance.
    const stretch = duration && sound.duration ? Math.max(0.05, duration) / sound.duration : 1;
    let longest = 0;
    const nodes = [];
    for (const layer of layersFor(sound, stage)) {
      const { span, tail } = this.playLayer(layer, at, level, pan, detune, note, stretch);
      longest = Math.max(longest, span);
      nodes.push(tail);
    }

    // A moment past the end, so nothing is unplugged while its own tail is
    // still ringing out.
    live.push({ endsAt: at + longest + 0.05, nodes });
    this.voices.set(name, live);
    return true;
  }

  /// Forgets the voices that have finished and unplugs what they were built
  /// from.
  ///
  /// The unplugging matters as much as the counting. Nothing else disconnects
  /// anything: a stopped source is eligible to be collected but its chain
  /// stays wired to the master bus until the browser gets round to it, and a
  /// level's worth of them is a great deal for a phone to be carrying. One
  /// disconnect per layer is enough, because a chain cut from the output is a
  /// chain nothing downstream has to look at.
  ///
  /// Driven from `play` rather than from a clock of its own. When nothing is
  /// being played there is nothing arriving to be late for, and the handful
  /// left connected through a silence costs nothing.
  sweep() {
    if (!this.ctx) {
      return;
    }
    const now = this.ctx.currentTime;
    for (const [name, live] of this.voices) {
      let kept = 0;
      for (const voice of live) {
        if (voice.endsAt > now) {
          live[kept] = voice;
          kept += 1;
          continue;
        }
        for (const node of voice.nodes) {
          node.disconnect();
        }
      }
      live.length = kept;
      if (kept === 0) {
        this.voices.delete(name);
      }
    }
  }

  /// Builds one layer's little graph and schedules it.
  ///
  /// Hands back how long it runs and the node on the end of it, which is what
  /// `sweep` unplugs from the master bus once it has.
  playLayer(layer, at, level, pan, detune, noteOverride, stretch = 1) {
    const ctx = this.ctx;
    const jitter = layer.jitter ?? {};
    const wobble = (amount) => (amount ? 1 + (Math.random() * 2 - 1) * amount : 1);

    // A layer may draw a random overtone of one root for its filters to tune
    // to. Free frequencies scattered across a cascade are noise; whole
    // multiples of a single fundamental are the harmonic series, and a pile of
    // them rings as one sound however many arrive. This sets no pitch of its
    // own: it is a frequency for a filter, not a note.
    let harmonicHz = null;
    if (layer.harmonic) {
      const root = noteToHz(layer.harmonic.of ?? A4);
      const from = Math.max(1, Math.round(layer.harmonic.from ?? 1));
      const to = Math.max(from, Math.round(layer.harmonic.to ?? from));
      harmonicHz = root * (from + Math.floor(Math.random() * (to - from + 1)));
    }

    // A layer opts out of stretching when it is a fixed event rather than part
    // of the body of the sound: an ignition hiss is the same length however
    // far the rocket is going.
    const span = layer.stretch === false ? 1 : stretch;

    // Oscillators all start at phase zero, so several copies fired at the same
    // instant sum coherently and peak in proportion to their number rather than
    // to the square root of it. Nudging each start by up to a cycle scatters
    // the phase; at these pitches that is a few milliseconds, which moves the
    // sound not at all and the peak a great deal.
    const nominalHz =
      layer.source === 'noise' ? 0 : noteToHz(noteOverride ?? layer.note ?? A4);
    const phase = nominalHz > 0 ? Math.min(0.008, Math.random() / nominalHz) : 0;
    const start = at + (layer.delay ?? 0) * span + phase;
    const env = layer.env ?? { attack: 0.002, decay: 0.1 };
    // The attack is left alone: a transient that stretches stops being one.
    const attack = Math.max(0.0005, env.attack ?? 0.002);
    const hold = Math.max(0, (env.hold ?? 0) * span);
    const decay = Math.max(0.005, (env.decay ?? 0.1) * span);
    const duration = attack + hold + decay;

    let source;
    if (layer.source === 'noise') {
      source = ctx.createBufferSource();
      source.buffer = this.noise;
      // Start somewhere random in the buffer so repeats are not identical.
      source.loop = true;
      source.loopStart = 0;
      source.loopEnd = this.noise.duration;
    } else {
      source = ctx.createOscillator();
      source.type = layer.source ?? 'sine';
      const hz = noteToHz(noteOverride ?? layer.note ?? A4) * wobble(jitter.frequency);
      const to = layer.sweep ? noteToHz(layer.sweep.to) * wobble(jitter.frequency) : null;
      const glide = Math.max(0.001, (layer.sweep?.time ?? duration) * span);

      source.frequency.setValueAtTime(hz, start);
      if (to !== null && layer.waver) {
        // Fireworks do not hold a pitch cleanly. Rather than a tidy vibrato,
        // walk the glide in small steps and push each one slightly off, so the
        // wobble never repeats.
        const rate = layer.waver.rate ?? 14;
        const depth = layer.waver.depth ?? 0.03;
        const steps = Math.max(2, Math.round(glide * rate));
        for (let i = 1; i <= steps; i += 1) {
          const t = i / steps;
          const along = hz * (to / hz) ** t;
          const off = 1 + (Math.random() * 2 - 1) * depth;
          source.frequency.linearRampToValueAtTime(Math.max(20, along * off), start + t * glide);
        }
      } else if (to !== null) {
        source.frequency.exponentialRampToValueAtTime(Math.max(1, to), start + glide);
      }
      source.detune.value = detune + (jitter.detune ? (Math.random() * 2 - 1) * 1200 * jitter.detune : 0);
    }

    let node = source;
    for (const spec of layer.filters ?? []) {
      const filter = ctx.createBiquadFilter();
      filter.type = spec.type ?? 'lowpass';
      // `frequency: 'harmonic'` tunes the filter to the overtone this play
      // drew, which is how a high-Q band-pass on noise becomes a pitch.
      const center = spec.frequency === 'harmonic' ? harmonicHz ?? 1000 : spec.frequency ?? 1000;
      const from = Math.max(20, center * wobble(jitter.frequency));
      filter.frequency.setValueAtTime(from, start);
      if (spec.sweep) {
        // A cutoff that falls as the sound decays is what makes air disperse
        // rather than simply stop.
        const to = Math.max(20, noteToHz(spec.sweep.to) * wobble(jitter.frequency));
        filter.frequency.exponentialRampToValueAtTime(
          to,
          start + Math.max(0.001, (spec.sweep.time ?? duration) * span),
        );
      }
      filter.Q.value = spec.q ?? 1;
      node.connect(filter);
      node = filter;
    }

    const amp = ctx.createGain();
    const peak = Math.max(0.0001, level * (layer.gain ?? 1) * wobble(jitter.gain));
    amp.gain.setValueAtTime(0.0001, start);
    amp.gain.linearRampToValueAtTime(peak, start + attack);
    if (hold > 0) {
      amp.gain.setValueAtTime(peak, start + attack + hold);
    }
    // Exponential, because a linear fade on a short percussive sound clicks.
    amp.gain.exponentialRampToValueAtTime(0.0001, start + duration);
    node.connect(amp);

    let tail = amp;
    const panning = (layer.pan ?? 0) + pan;
    if (panning !== 0 && ctx.createStereoPanner) {
      const panner = ctx.createStereoPanner();
      panner.pan.value = Math.max(-1, Math.min(1, panning));
      amp.connect(panner);
      tail = panner;
    }
    tail.connect(this.master);

    source.start(start, layer.source === 'noise' ? Math.random() * 1.5 : undefined);
    source.stop(start + duration + 0.01);
    return { span: (layer.delay ?? 0) * span + duration + phase, tail };
  }
}

/// The layers one play of a sound needs.
///
/// Most sounds simply list theirs. A sound built from `chords` instead names a
/// list of note lists and a single `voice`, and the stage picks which chord to
/// spread across that voice, one definition covering a whole progression. A
/// stage past the end holds on the last chord rather than wrapping back to the
/// bottom, so a very long chain stays at its peak instead of collapsing.
function layersFor(sound, stage) {
  if (!sound.chords || !sound.voice) {
    return sound.layers ?? [];
  }
  const index = Math.min(sound.chords.length - 1, Math.max(0, Math.floor(stage)));
  return sound.chords[index].map((note) => ({ ...sound.voice, note }));
}

/// A couple of seconds of white noise, made once and shared by every noise
/// layer, each of which reads from a random offset.
function makeNoiseBuffer(ctx, seconds = 2) {
  const frames = Math.floor(ctx.sampleRate * seconds);
  const buffer = ctx.createBuffer(1, frames, ctx.sampleRate);
  const data = buffer.getChannelData(0);
  for (let i = 0; i < frames; i += 1) {
    data[i] = Math.random() * 2 - 1;
  }
  return buffer;
}
