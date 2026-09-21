// Sound definitions.
//
// A sound is a stack of layers, each one an oscillator or a burst of noise
// shaped by an envelope. Layers carry their own pitch and their own offset, so
// a chord is several layers at the same moment and an arpeggio is the same
// layers a few milliseconds apart. Everything here is data — see audio.js for
// what plays it.
//
//   source    'noise', or an oscillator: 'sine' 'square' 'sawtooth' 'triangle'
//   note      pitch, as a name ('C5', 'F#4', 'Bb3') or a number in hertz
//   sweep     { to, time }   pitch glides there over that many seconds
//   filters   [{ type, frequency, q, sweep }]  in order, as on a BiquadFilterNode;
//             sweep is { to, time } and glides the cutoff, which is how a
//             sound gets darker as it fades instead of just getting quieter
//   env       { attack, hold, decay }    seconds; decay falls away exponentially
//   gain      layer level, 0..1
//   delay     seconds to wait before this layer starts
//   pan       -1 left to 1 right, added to whatever the caller asks for
//   jitter    { frequency, gain, detune } fractional wobble applied per play,
//             so repeats of the same sound do not phase into one tone
//   waver     { depth, rate } walks the pitch glide in small steps, each one
//             pushed slightly off, the way a firework fails to hold its note
//   stretch   false to keep a layer at a fixed length when the sound as a whole
//             is stretched to fill a duration the caller asked for
//
// A sound may declare `duration`, its natural length in seconds. Playing it
// with a `duration` option then scales the holds, decays and glides to fit —
// attacks are left alone, because a transient that stretches is not one.
//
// A sound may instead declare `chords` and a `voice`: a list of note lists, and
// the single layer each note is played through. Playing it with a `stage` picks
// a chord, so one definition covers a whole progression.
//
// Keep levels low. These stack: a rainbow clear can fire twenty at once, and
// the limiter should be a safety net rather than something the game leans on.

export const SOUNDS = {
  /// A gem going away: a soft "tff", like a hi-hat brushed rather than struck.
  /// Deliberately quiet and very short so a cascade reads as a texture.
  pop: {
    gain: 0.5,
    layers: [
      {
        // Band-limited air with a soft onset. No transient spike and no pitch:
        // either one turns this from a "tff" into a click.
        source: 'noise',
        filters: [
          { type: 'highpass', frequency: 340, q: 0.6 },
          { type: 'lowpass', frequency: 4200, q: 0.9, sweep: { to: 680, time: 0.075 } },
        ],
        env: { attack: 0.007, decay: 0.08 },
        gain: 1,
        jitter: { frequency: 0.18, gain: 0.25 },
      },
    ],
  },

  /// A rocket reaching its target: a low boom with debris behind it.
  ///
  /// The body is kept above roughly 70Hz on purpose. A phone speaker cannot
  /// move air below about 200Hz, so energy underneath that is spent on nothing
  /// — it eats headroom and arrives as silence on the device most people will
  /// play this on. What makes a boom read small is the low-mid behind it.
  boom: {
    gain: 0.46,
    voiceCap: 4,
    layers: [
      {
        source: 'sine',
        note: 190,
        sweep: { to: 72, time: 0.25 },
        env: { attack: 0.003, decay: 0.62 },
        gain: 1,
        jitter: { frequency: 0.08, gain: 0.12 },
      },
      {
        // Debris, thinning as it falls away.
        source: 'noise',
        filters: [
          { type: 'lowpass', frequency: 2000, q: 0.7, sweep: { to: 220, time: 0.3 } },
          { type: 'highpass', frequency: 110, q: 0.5 },
        ],
        env: { attack: 0.004, decay: 0.66 },
        gain: 0.55,
        jitter: { frequency: 0.15, gain: 0.2 },
      },
      {
        // A short crack on the front, so it lands as an impact rather than
        // swelling up out of nowhere.
        source: 'noise',
        filters: [{ type: 'bandpass', frequency: 1100, q: 0.8, sweep: { to: 340, time: 0.06 } }],
        env: { attack: 0.0015, decay: 0.08 },
        gain: 0.5,
        jitter: { frequency: 0.2, gain: 0.2 },
      },
    ],
  },

  /// The musical payoff of a clear, one chord per step of the chain.
  ///
  /// A chain starts at the first chord and climbs as it goes, so a long cascade
  /// walks up the scale and the player hears how well they did. Twelve steps in
  /// F; past that it holds at the top rather than running out of room.
  ///
  /// Plucked rather than sung: a hard attack, a fast decay, and a lowpass
  /// closing as the note falls away, which is what a sawtooth needs to stop
  /// sounding like a buzzer.
  chime: {
    gain: 0.2,
    voiceCap: 6,
    chords: [
      ['A3', 'C4', 'F4'],
      ['C4', 'E4', 'G4'],
      ['C4', 'F4', 'A4'],
      ['D4', 'F4', 'Bb4'],
      ['F4', 'A4', 'C5'],
      ['G4', 'Bb4', 'D5'],
      ['Bb4', 'C5', 'E5'],
      ['A4', 'C5', 'F5'],
      ['C5', 'E5', 'G5'],
      ['C5', 'F5', 'A5'],
      ['E5', 'G5', 'C6'],
      ['A5', 'C6', 'F6'],
    ],
    voice: {
      source: 'sawtooth',
      filters: [
        { type: 'lowpass', frequency: 2400, q: 1.1, sweep: { to: 620, time: 0.18 } },
        { type: 'highpass', frequency: 120, q: 0.5 },
      ],
      env: { attack: 0.002, decay: 0.415 },
      gain: 1,
      jitter: { gain: 0.1 },
    },
  },

  /// A rocket in flight: ignition, then a whistle falling away until it hits.
  ///
  /// The whistle sustains rather than decaying, because it has to still be
  /// there when the boom lands. Its length is the flight time, which the engine
  /// works out from the distance and passes in, so a shot across the board
  /// whistles for longer than one next door.
  rocket: {
    gain: 0.3,
    voiceCap: 4,
    duration: 0.9,
    layers: [
      {
        // Ignition: a short hiss, the same however far it is going.
        source: 'noise',
        stretch: false,
        filters: [
          { type: 'highpass', frequency: 900, q: 0.6 },
          { type: 'lowpass', frequency: 7500, q: 0.7, sweep: { to: 2200, time: 0.14 } },
        ],
        env: { attack: 0.005, decay: 0.15 },
        gain: 0.8,
        jitter: { frequency: 0.15, gain: 0.15 },
      },
      {
        // The whistle, high and slowly falling, never quite holding its pitch.
        // Depth is a fraction of the frequency, so 0.022 is about a third of a
        // semitone either way — a waver, not a vibrato.
        source: 'triangle',
        note: 2050,
        sweep: { to: 720, time: 0.86 },
        waver: { depth: 0.022, rate: 16 },
        env: { attack: 0.05, hold: 0.77, decay: 0.08 },
        gain: 0.5,
        delay: 0.03,
        jitter: { frequency: 0.07, gain: 0.15 },
      },
      {
        // Air rushing past it, tracking the same fall.
        source: 'noise',
        filters: [{ type: 'bandpass', frequency: 2800, q: 2.5, sweep: { to: 1000, time: 0.86 } }],
        env: { attack: 0.09, hold: 0.7, decay: 0.1 },
        gain: 0.14,
        jitter: { frequency: 0.12 },
      },
    ],
  },
};
