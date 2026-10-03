// Sound definitions.
//
// A sound is a stack of layers, each one an oscillator or a burst of noise
// shaped by an envelope. Layers carry their own pitch and their own offset, so
// a chord is several layers at the same moment and an arpeggio is the same
// layers a few milliseconds apart. Everything here is data. See audio.js for
// what plays it.
//
//   source    'noise', or an oscillator: 'sine' 'square' 'sawtooth' 'triangle'
//   note      pitch, as a name ('C5', 'F#4', 'Bb3') or a number in hertz
//   sweep     { to, time }   pitch glides there over that many seconds
//             { by, time }   the same, as an interval in cents from whatever
//             this layer is playing: -200 is a whole tone down. For a voice
//             whose note arrives from outside, where a fixed destination would
//             slide every note in a figure to the same pitch
//             { from, time }  the other direction: the note is where the glide
//             *ends*, and it begins that interval away. -150 scoops up into
//             the note from a tone and a half below, the way a slide arrives,
//             and the note is in tune for all of itself except the approach
//             `time` is optional and defaults to the layer's whole envelope,
//             which is what to leave it as unless the pitch is meant to settle
//             and hold. A glide that ends early goes flat for the rest of the
//             sound, and so does any `waver` riding on it, since the waver is
//             steps along the glide and there are none once it has arrived
//   vibrato   { depth, rate, after } a steady swing of that many cents, that
//             many times a second, starting that many seconds in and running
//             to the end of the note. For a held note that is leaned on, where
//             `waver` is for a pitch that never settles at all. `after` is in
//             seconds and is not stretched, so short notes never reach it and
//             one voice can play a figure where only the long note wavers
//   filters   [{ type, frequency, q, sweep }]  in order, as on a BiquadFilterNode;
//             sweep is { to, time } and glides the cutoff, which is how a
//             sound gets darker as it fades instead of just getting quieter
//   env       { attack, hold, decay }    seconds; decay falls away exponentially
//   gain      layer level, 0..1
//   detune    cents, added to whatever the caller asks for. How one voice
//             holds more than one pitch when the note is handed to it from
//             outside: 1200 is the octave above, and a few cents is the
//             shimmer that makes two oscillators read as one wide one
//   delay     seconds to wait before this layer starts
//   pan       -1 left to 1 right, added to whatever the caller asks for
//   jitter    { frequency, gain, detune } fractional wobble applied per play,
//             so repeats of the same sound do not phase into one tone
//   waver     { depth, rate } walks the pitch glide in small steps, each one
//             pushed slightly off, the way a firework fails to hold its note
//   stretch   false to keep a layer at a fixed length when the sound as a whole
//             is stretched to fill a duration the caller asked for
//   harmonic  { of, from, to } draws a random whole multiple of one root note
//             per play, between those two multiples. Any filter whose frequency
//             is the string 'harmonic' tunes to it. It is a filter frequency,
//             not a pitch: free random frequencies scattered across a cascade
//             are noise, while overtones of one fundamental are a chord
//
// A sound may declare `scatter`, in seconds: each play is held back by a random
// moment up to that long, so a pile of them fired at one instant spreads out
// instead of landing as a single event.
//
// A sound may declare `duration`, its natural length in seconds. Playing it
// with a `duration` option then scales the holds, decays and glides to fit.
// Attacks are left alone, because a transient that stretches is not one.
//
// A sound may instead declare `chords` and a `voice`: a list of note lists, and
// the single layer each note is played through. Playing it with a `stage` picks
// a chord, so one definition covers a whole progression.
//
// A sound meant to be played as a note in a written figure declares `duration`
// and leaves its own `note` off every layer, because the pitch arrives per note
// from the tune rather than being part of the sound. `keys`, `glass` and `bass`
// are the three of those, and `Audio.sequence` is what plays them.
//
// Keep levels low. These stack: a rainbow clear can fire twenty at once, and
// the limiter should be a safety net rather than something the game leans on.

/// The rattle inside the shuffle, as (start in seconds, band center in hertz,
/// level). A riffle is one gesture made of a dozen tiny strikes, so this is a
/// table rather than a dozen near-identical layers written out: the numbers are
/// the whole design and this way they can be read down a column.
///
/// The gaps tighten to the middle and open out again, which is a shuffle
/// gathering speed and then settling. The centers drift downward over the same
/// stretch, so the rattle darkens as it runs out.
export const RIFFLE = [
  [0.0, 2900, 0.5],
  [0.034, 2400, 0.62],
  [0.064, 3050, 0.7],
  [0.09, 2200, 0.78],
  [0.112, 2750, 0.82],
  [0.132, 2050, 0.8],
  [0.15, 2600, 0.76],
  [0.17, 1900, 0.7],
  [0.194, 2350, 0.62],
  [0.222, 1750, 0.55],
  [0.256, 2100, 0.48],
  [0.298, 1600, 0.4],
  [0.348, 1850, 0.32],
  [0.408, 1450, 0.24],
];

/// The figure the fanfare plays, as (note, start in seconds, level, how long
/// it rings). A major triad walked up and landing on the octave.
///
/// A table for the same reason the riffle is one: these numbers are the whole
/// design, and the figure can be rewritten by moving them down a column rather
/// than by editing five near-identical layers.
export const FANFARE = [
  ['G4', 0.0, 0.5, 0.26],
  ['C5', 0.09, 0.55, 0.26],
  ['E5', 0.18, 0.6, 0.26],
  ['G5', 0.27, 0.66, 0.34],
  ['C6', 0.42, 0.8, 0.95],
];

// Steamboat Willie
export const STEAMBOAT = {
  tempo: 168,
  melody: [
    // lead-in
    ['C4', 0, 1],
    // bar 1
    ['G#4', 1, 0.5],
    ['A4', 1.5, 1],
    ['F4', 2.5, 1],
    ['G4', 3.5, 1],
    ['G#4', 4.5, 0.5],
    // bar 2
    ['A4', 5, 0.5],
    ['G#4', 5.5, 0.5],
    ['A4', 6, 0.5],
    ['G#4', 6.5, 0.5],
    ['A4', 7, 0.5],
    ['C5', 7.5, 1],
    // bar 3
    ['G#4', 9, 0.5],
    ['A4', 9.5, 1],
    ['F4', 10.5, 1.5],
    ['A4', 12.5, 0.5],
    // bar 4
    ['C5', 13, 0.5],
    ['D5', 13.5, 0.5],
    ['C5', 14, 0.5],
    ['A4', 14.5, 0.5],
    ['G4', 15, 1],
    ['B4', 15.88, 0.25],
    ['C4', 16, 1],
    // bar 5
    ['G#4', 17, 0.5],
    ['A4', 17.5, 1],
    ['F4', 18.5, 1],
    ['G4', 19.5, 1],
    ['G#4', 20.5, 0.5],
    // bar 6
    ['A4', 21, 0.5],
    ['G#4', 21.5, 0.5],
    ['A4', 22, 0.5],
    ['G#4', 22.5, 0.5],
    ['A4', 23, 0.5],
    ['C5', 23.5, 1],
    // bar 7
    ['D5', 25, 0.5],
    ['D5', 25.5, 0.5],
    ['D5', 26, 0.5],
    ['D5', 26.5, 0.5],
    ['C5', 27, 0.5],
    ['A4', 27.5, 0.5],
    ['F4', 28, 0.5],
    ['G4', 28.5, 0.5],
    // bar 8
    ['A4', 29, 0.5],
    ['F4', 29.5, 0.5],
    ['G4', 30, 0.5],
    ['F4', 30.5, 1],
  ],

  harmony: [
    ['F4', 2, 1],
    ['F4', 4, 1],
    ['F4', 6, 1],
    ['F4', 8, 1],
    ['F4', 10, 1],
    ['F4', 12, 1],
    ['F4', 14, 1],
    ['E4', 16, 1],
    ['F4', 18, 1],
    ['F4', 20, 1],
    ['F4', 22, 1],
    ['F4', 24, 1],
    ['F4', 25, 0.5],
    ['F4', 25.5, 0.5],
    ['F4', 26, 0.5],
    ['F4', 26.5, 0.5],
    ['F4', 28, 1],
    ['C5', 29, 1],
    ['Bb4', 30, 1],
    ['A4', 31, 1],
  ],

  bass: [
    ['F3', 1, 1],
    ['C3', 3, 1],
    ['F3', 5, 1],
    ['C3', 7, 1],
    ['F3', 9, 1],
    ['C3', 11, 1],
    ['F3', 13, 1],
    ['C3', 15, 1],
    ['F3', 17, 1],
    ['C3', 19, 1],
    ['F3', 21, 1],
    ['C3', 23, 1],
    ['Bb3', 25, 1],
    ['C4', 27, 1],
    ['C4', 29, 1],
    ['C3', 30, 1],
    ['F3', 31, 1],
  ],
};

/// The parts of [`STEAMBOAT`], ready to hand to `Audio.sequence`.
///
/// The two melodic voices are spread a little apart rather than both up the
/// middle, which is most of what stops three parts reading as one thick one.
export const VICTORY_PARTS = [
  { sound: 'keys', notes: STEAMBOAT.melody, pan: -0.12 },
  { sound: 'glass', notes: STEAMBOAT.harmony, pan: 0.2, gain: 0.85 },
  { sound: 'bass', notes: STEAMBOAT.bass },
];

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

  /// A swap that came to nothing: two quick wooden knocks, high then low.
  ///
  /// A woodblock is a short pitched body with a click on the front, so each
  /// knock is a triangle dropping a little in pitch as it dies, with a narrow
  /// band of noise for the strike itself. Unlike every other sound here, the
  /// click is wanted: this one is meant to sound like a thing being hit.
  ///
  /// The two land a breath apart, which is what makes it read as a rattle
  /// rather than a single knock.
  clack: {
    gain: 0.34,
    voiceCap: 3,
    layers: [
      {
        source: 'triangle',
        note: 1180,
        sweep: { to: 880, time: 0.02 },
        env: { attack: 0.001, decay: 0.038 },
        gain: 0.85,
        jitter: { frequency: 0.03, gain: 0.12 },
      },
      {
        source: 'noise',
        filters: [{ type: 'bandpass', frequency: 2500, q: 5 }],
        env: { attack: 0.0008, decay: 0.022 },
        gain: 0.55,
        jitter: { frequency: 0.12 },
      },
      {
        source: 'triangle',
        note: 790,
        sweep: { to: 600, time: 0.022 },
        env: { attack: 0.001, decay: 0.06 },
        delay: 0.072,
        gain: 0.9,
        jitter: { frequency: 0.03, gain: 0.12 },
      },
      {
        source: 'noise',
        filters: [{ type: 'bandpass', frequency: 1750, q: 5 }],
        env: { attack: 0.0008, decay: 0.024 },
        delay: 0.072,
        gain: 0.55,
        jitter: { frequency: 0.12 },
      },
    ],
  },

  /// A column of gems touching down after gravity: a soft, low thud.
  ///
  /// A handful of these land within a fraction of a second of each other every
  /// time the board collapses, so it is quiet and short: it is meant to give
  /// the fall a floor to hit, not to be an event in itself.
  ///
  /// The body falls in pitch rather than holding one, because an impact
  /// decelerates and a fixed tone reads as a note. Most of what a phone will
  /// actually reproduce is the knock above it: the body's tail ends up under
  /// what a small speaker can move, the same trade the boom makes.
  thud: {
    // Trimmed from 0.26 when the board went from eight columns to nine: nine
    // of these landing together clear the limiter's threshold where eight sat
    // under it. This is the one level in here set by arithmetic rather than by
    // ear, and it is worth an ear before it is trusted.
    gain: 0.22,
    // A whole board settling is one landing per column, all at once, and
    // thinning that would drop thuds the player can hear are missing. Nine is
    // the board's width; a wider board wants this raised with it.
    voiceCap: 9,
    layers: [
      {
        source: 'sine',
        note: 120,
        sweep: { to: 64, time: 0.055 },
        env: { attack: 0.004, decay: 0.13 },
        gain: 1,
        jitter: { frequency: 0.12, gain: 0.2 },
      },
      {
        // The contact itself, dull and gone almost at once. Rolled off below
        // the body so the two do not pile up in the same octave.
        source: 'noise',
        filters: [
          { type: 'lowpass', frequency: 900, q: 0.7, sweep: { to: 260, time: 0.05 } },
          { type: 'highpass', frequency: 150, q: 0.5 },
        ],
        env: { attack: 0.002, decay: 0.055 },
        gain: 0.4,
        jitter: { frequency: 0.2, gain: 0.25 },
      },
    ],
  },

  /// The board rearranging itself when nothing can be matched: a riffle.
  ///
  /// Shuffling a deck is the sound everyone already knows for this, and it is
  /// a good fit: a burr of small strikes that speeds up, then spreads out and
  /// stops. Underneath runs a soft sweep down, which is the movement itself
  /// rather than any one gem, and a low tock closes it as the board lands.
  ///
  /// Nothing else is playing while this happens, so it can afford to be the
  /// only thing in the mix.
  shuffle: {
    gain: 0.42,
    voiceCap: 2,
    // Its natural length: the closing tock lands at 0.46 and rings for 0.12.
    // Declared so the page can stretch the whole gesture to the length of the
    // animation rather than this sound carrying a length of its own, which is
    // the same arrangement the rocket's whistle has with its flight time. The
    // rattle spreads out with it, which is what doubling it is for: the
    // strikes are the gesture, and a gesture played at the old speed under a
    // slower animation finishes while the board is still moving.
    duration: 0.58,
    layers: [
      {
        // The gesture under the strikes, darkening as the board settles.
        source: 'noise',
        filters: [
          { type: 'bandpass', frequency: 900, q: 1.1, sweep: { to: 380, time: 0.5 } },
        ],
        env: { attack: 0.06, hold: 0.16, decay: 0.3 },
        gain: 0.3,
        jitter: { frequency: 0.12, gain: 0.15 },
      },
      ...RIFFLE.map(([delay, frequency, gain]) => ({
        source: 'noise',
        filters: [{ type: 'bandpass', frequency, q: 5 }],
        env: { attack: 0.0006, decay: 0.022 },
        delay,
        gain,
        // Enough per play that two shuffles in a row are not the same sound
        // twice, which a fixed table would otherwise guarantee.
        jitter: { frequency: 0.14, gain: 0.22 },
      })),
      {
        // The board coming to rest, a beat after the rattle runs out.
        source: 'triangle',
        note: 232,
        sweep: { to: 158, time: 0.05 },
        env: { attack: 0.003, decay: 0.12 },
        delay: 0.46,
        gain: 0.42,
        jitter: { frequency: 0.06, gain: 0.15 },
      },
    ],
  },

  /// The move budget running short: like a doorbell,
  ///
  ding: {
    gain: 0.5,
    voiceCap: 2,
    layers: [
      {
        source: 'triangle',
        note: 'F#6',
        filters: [{ type: 'lowpass', frequency: 4600, q: 0.8, sweep: { to: 1500, time: 0.36 } }],
        env: { attack: 0.004, decay: 0.48 },
        gain: 0.9,
        jitter: { frequency: 0.015, gain: 0.08 },
      },
      {
        source: 'sine',
        note: 'B5',
        env: { attack: 0.004, decay: 0.2 },
        gain: 0.2,
        jitter: { gain: 0.1 },
      },
      {
        source: 'triangle',
        note: 'G6',
        filters: [{ type: 'lowpass', frequency: 4600, q: 0.8, sweep: { to: 1500, time: 0.36 } }],
        env: { attack: 0.004, decay: 1.48 },
        delay: 0.05,
        gain: 0.9,
        jitter: { frequency: 0.015, gain: 0.08 },
      },
      {
        source: 'sine',
        note: 'C6',
        env: { attack: 0.004, decay: 1.2 },
        delay: 0.05,
        gain: 0.2,
        jitter: { gain: 0.1 },
      },
      {
        source: 'triangle',
        note: 'C6',
        filters: [{ type: 'lowpass', frequency: 4200, q: 0.8, sweep: { to: 1300, time: 0.5 } }],
        env: { attack: 0.004, decay: 1.72 },
        delay: 0.5,
        gain: 0.95,
        jitter: { frequency: 0.015, gain: 0.08 },
      },
      {
        source: 'sine',
        note: 'G5',
        env: { attack: 0.004, decay: 1.28 },
        delay: 0.5,
        gain: 0.2,
        jitter: { gain: 0.1 },
      },
    ],
  },

  /// A brick taking a hit: a dry stony crack with grit falling after it.
  ///
  /// Nothing else on the board is made of stone, so this leans on the one thing
  /// gems never do: a hard, toneless snap with no pitch to hang on to, and a
  /// short rattle of debris behind it. Played harder when the brick actually
  /// breaks, which the caller does with `gain`.
  crack: {
    gain: 0.4,
    voiceCap: 4,
    layers: [
      {
        // The snap. Band-limited noise rather than a tone: stone has no note.
        source: 'noise',
        filters: [
          { type: 'highpass', frequency: 700, q: 0.7 },
          { type: 'bandpass', frequency: 1900, q: 1.6, sweep: { to: 900, time: 0.05 } },
        ],
        env: { attack: 0.0008, decay: 0.07 },
        gain: 1,
        jitter: { frequency: 0.2, gain: 0.2 },
      },
      {
        // The weight behind it, so it lands as masonry and not as a twig.
        source: 'triangle',
        note: 210,
        sweep: { to: 120, time: 0.05 },
        env: { attack: 0.002, decay: 0.1 },
        gain: 0.5,
        jitter: { frequency: 0.12, gain: 0.2 },
      },
      {
        // Grit, arriving just behind the break rather than with it.
        source: 'noise',
        filters: [{ type: 'bandpass', frequency: 3200, q: 1.1, sweep: { to: 1500, time: 0.18 } }],
        env: { attack: 0.004, decay: 0.2 },
        delay: 0.035,
        gain: 0.3,
        jitter: { frequency: 0.18, gain: 0.25 },
      },
    ],
  },

  /// The shimmer left behind by a gem, ringing on long after the pop.
  ///
  /// The note underneath never changes: a sawtooth held at a low F, which has
  /// energy at every whole multiple of itself. What varies is where the
  /// band-pass listens (a different overtone of that one fundamental each
  /// time), so every gem picks out a real partial of the same note and twenty
  /// of them ring as one chord rather than a scatter of unrelated tones.
  ///
  /// (Swapping `source` to 'noise' gives the airier version of the same idea:
  /// the filter rings on its own and the noise merely feeds it.)
  ///
  /// Very quiet, because these pile up several seconds deep.
  sparkle: {
    // Judge this by ear rather than by its peak. A band-passed tone ringing
    // for over a second sits far higher in the mix than its peak suggests:
    // it is sustained, narrow, and right where hearing is sharpest, where a
    // pop is a broadband tick lasting sixty milliseconds.
    gain: 0.05,
    voiceCap: 16,
    // Every gem in a clear asks for one of these at the same instant. Spreading
    // their starts across a fifth of a second turns a single chime into
    // something that twinkles.
    scatter: 0.2,
    layers: [
      {
        source: 'sawtooth',
        note: 'F4',
        harmonic: { of: 'F1', from: 16, to: 36 },
        filters: [
          // Two passes at the same center: one band-pass this resonant still
          // leaks noise around the skirts, and a second cleans it into a tone.
          { type: 'bandpass', frequency: 'harmonic', q: 34 },
          { type: 'bandpass', frequency: 'harmonic', q: 16 },
        ],
        env: { attack: 0.01, decay: 1.5 },
        gain: 1,
        jitter: { gain: 0.35 },
      },
    ],
  },

  /// A rocket reaching its target: a low boom with debris behind it.
  ///
  /// The body is kept above roughly 70Hz on purpose. A phone speaker cannot
  /// move air below about 200Hz, so energy underneath that is spent on
  /// nothing: it eats headroom and arrives as silence on the device most
  /// people will play this on. What makes a boom read small is the low-mid
  /// behind it.
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
  /// A leftover move being spent at the end of a level: one struck bell per
  /// gem, whether or not a special lands on it.
  ///
  /// Bell-like comes from the partials, not the fundamental. A struck bell
  /// rings a stack of overtones that fade at different rates, the high ones
  /// first, so what starts bright settles into a hum. Sines rather than a
  /// filtered sawtooth because the interesting part is which partials are
  /// there and how long each lasts, and that is easier to read written out
  /// than inferred from a filter sweep.
  ///
  /// In F, like the rest of the mix. The fifth above sits under the octave
  /// slightly detuned, which is the wobble a real bell has and the reason two
  /// struck together never sound like one tone.
  ///
  /// The numbers below are a first pass at the shape, not a settled sound.
  bell: {
    gain: 0.16,
    // Several are ringing at once: at 300ms apart and a second of decay,
    // three or four overlap by design.
    voiceCap: 6,
    layers: [
      // The strike itself. Almost nothing, but without it the bell fades up
      // rather than being hit.
      {
        source: 'noise',
        filters: [{ type: 'highpass', frequency: 3200, q: 0.7 }],
        env: { attack: 0.001, decay: 0.035 },
        gain: 0.22,
      },
      // The hum: what is left a second later.
      {
        source: 'sine',
        note: 'F4',
        env: { attack: 0.002, decay: 1.25 },
        gain: 0.5,
        jitter: { frequency: 0.004, gain: 0.12 },
      },
      // The fifth, detuned enough to beat gently against the octave.
      {
        source: 'sine',
        note: 'C5',
        env: { attack: 0.002, decay: 0.78 },
        gain: 0.3,
        jitter: { frequency: 0.007, gain: 0.15 },
      },
      // The octave, and above it the partial that makes it read as metal
      // rather than as a flute. Both fade first, which is the whole envelope.
      {
        source: 'sine',
        note: 'F5',
        env: { attack: 0.001, decay: 0.5 },
        gain: 0.26,
        jitter: { frequency: 0.006, gain: 0.15 },
      },
      {
        source: 'sine',
        note: 'C6',
        env: { attack: 0.001, decay: 0.3 },
        gain: 0.16,
        jitter: { frequency: 0.01, gain: 0.2 },
      },
    ],
  },

  chime: {
    gain: 0.4,
    voiceCap: 6,
    chords: [
      ['A3', 'C4', 'F4'],
      ['C4', 'E4', 'G4'],
      ['C4', 'F4', 'A4'],
      ['D4', 'F4', 'Bb4'],
      ['F4', 'G4', 'C5'],
      ['Bb4', 'C5', 'D5'],
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
        { type: 'lowpass', frequency: 2400, q: 1.1, sweep: { to: 20, time: 0.68 } },
        { type: 'highpass', frequency: 120, q: 0.5 },
      ],
      env: { attack: 0.002, decay: 0.915 },
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
        env: { attack: 0.105, decay: 0.85 },
        gain: 0.8,
        jitter: { frequency: 0.15, gain: 0.15 },
      },
      {
        // The whistle, high and slowly falling, never quite holding its pitch.
        // Depth is a fraction of the frequency, so 0.022 is about a third of a
        // semitone either way: a waver, not a vibrato.
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

  /// A level cleared, sounded with the toast that says so.
  ///
  /// **A placeholder.** Deliberately the plainest thing that reads as a
  /// fanfare: a triad walked up and landing on the octave, a bright source
  /// with the top taken off it as it rings, and a low note under the whole
  /// figure so the notes sit on something instead of arriving as five separate
  /// beeps. The figure is [`FANFARE`], which is where to rewrite it.
  fanfare: {
    gain: 0.45,
    // One at a time. A level is cleared once, and two of these over each other
    // would be a fault somewhere rather than something to hear.
    voiceCap: 1,
    layers: [
      {
        // The body under the figure. It holds through the run up and fades
        // with the last note rather than under it.
        source: 'sine',
        note: 'C4',
        env: { attack: 0.02, hold: 0.42, decay: 0.8 },
        gain: 0.2,
        jitter: { gain: 0.08 },
      },
      ...FANFARE.map(([note, delay, gain, decay]) => ({
        // Bright to start with and darkening as it goes, which is most of what
        // separates a horn from a beep.
        source: 'sawtooth',
        note,
        filters: [{ type: 'lowpass', frequency: 3000, q: 0.9, sweep: { to: 1100, time: decay } }],
        env: { attack: 0.012, hold: 0.03, decay },
        delay,
        gain,
        jitter: { frequency: 0.008, gain: 0.06 },
      })),
    ],
  },

  // ---- the three voices a written figure is played through ----
  //
  // None of these carries a note. A voice is an instrument rather than a
  // sound: the pitch arrives per note from the tune, and `duration` is what
  // one note is worth at its natural length, so holding it longer stretches
  // the ring rather than needing a second definition. See [`STEAMBOAT`].
  //
  // Cheerful on purpose, and nearer a slot machine than a piano: fast attacks,
  // a bright top that darkens as each note rings, and no noise anywhere. The
  // percussion a fanfare would use is deliberately absent, because three clean
  // voices carry a tune and a drum only covers it up.

  /// The melody. Two triangles a few cents apart for width, with a square an
  /// octave over them, short enough to be the strike rather than a note of its
  /// own.
  keys: {
    gain: 0.42,
    duration: 0.36,
    layers: [
      {
        source: 'triangle',
        filters: [{ type: 'lowpass', frequency: 4200, q: 0.8, sweep: { to: 900, time: 0.3 } }],
        env: { attack: 0.004, hold: 0.02, decay: 0.33 },
        gain: 0.85,
      },
      {
        source: 'triangle',
        // Seven cents, which is a fifteenth of a semitone: not a chord and not
        // an out of tune note, just enough beating to give the pair a body a
        // single oscillator does not have.
        detune: 7,
        filters: [{ type: 'lowpass', frequency: 4200, q: 0.8, sweep: { to: 900, time: 0.3 } }],
        env: { attack: 0.004, hold: 0.02, decay: 0.33 },
        // Well under the layer above rather than matched with it, which is a
        // level decision and not a taste one. Seven cents apart is a beat of
        // about two hertz, so a note shorter than half that beat is either
        // reinforced or canceled for its whole length depending on the phase
        // it happened to start on, and every oscillator here starts on a
        // random one. Two matched layers made the melody's level a coin flip
        // per note: measured across the tune it moved by a quarter between
        // renders and twice came out under the bassline. Carrying the note on
        // one oscillator and using the second only for shimmer keeps the width
        // and takes the swing out.
        gain: 0.3,
      },
      {
        source: 'square',
        detune: 1200,
        filters: [{ type: 'lowpass', frequency: 5200, q: 0.7, sweep: { to: 1600, time: 0.12 } }],
        env: { attack: 0.002, decay: 0.11 },
        gain: 0.2,
        // A transient that stretches stops being one, so a held note gets a
        // longer ring and the same strike.
        stretch: false,
      },
    ],
  },

  /// The second voice, under the melody's held notes. Rounder and softer, so
  /// it fills in behind rather than competing for the tune.
  glass: {
    gain: 0.22,
    duration: 0.36,
    layers: [
      {
        source: 'sine',
        env: { attack: 0.006, hold: 0.03, decay: 0.4 },
        gain: 0.9,
      },
      {
        source: 'triangle',
        detune: 1200,
        filters: [{ type: 'lowpass', frequency: 3200, q: 0.7, sweep: { to: 1200, time: 0.3 } }],
        env: { attack: 0.004, decay: 0.26 },
        gain: 0.3,
      },
    ],
  },

  /// The bassline. Dark, and cut off well below the melody so the two never
  /// argue about the middle of the mix.
  bass: {
    gain: 0.26,
    duration: 0.36,
    layers: [
      {
        source: 'triangle',
        filters: [{ type: 'lowpass', frequency: 700, q: 0.9, sweep: { to: 180, time: 0.3 } }],
        env: { attack: 0.005, hold: 0.04, decay: 0.3 },
        gain: 0.95,
      },
      {
        // The edge on the front of the note, which is what makes a bass
        // audible on a phone that cannot reproduce its fundamental at all.
        source: 'square',
        filters: [{ type: 'lowpass', frequency: 620, q: 0.8, sweep: { to: 260, time: 0.14 } }],
        env: { attack: 0.003, decay: 0.12 },
        gain: 0.16,
        stretch: false,
      },
    ],
  },

  // The named filler, below. See `NOISES` at the foot of this file for which
  // item plays which of these, and how.
  //
  // These are the only sounds here that are not feedback. Everything above
  // tells the player what the board just did, so it has to be short, quiet and
  // out of the way; a noise is the whole of what an item is, it happens once
  // and nothing is waiting on it, so it can afford to take a second and be the
  // only thing in the mix. Several arriving at once is still worth capping,
  // because a multiworld can hand over a fistful.

  /// Three knuckles on a wooden door, as (start in seconds, level).
  ///
  /// Built from the thud's shape rather than the clack's, which was the whole
  /// of what was wrong with the first attempt at this: a clack is a woodblock,
  /// a small solid thing struck and ringing, and a door is a large hollow one
  /// that thumps and stops. The clack's bright contact click was doing most of
  /// the damage, because the one thing a knock on a door has no trace of is a
  /// click.
  ///
  /// So each strike is three sine partials gliding down together, over a dull
  /// band of noise for the knuckle itself. Three and not one because a panel
  /// has modes rather than a pitch, and they are deliberately not harmonic:
  /// whole multiples of one root are a note, and a door is not a note. Gliding
  /// down because a struck panel loses tension as it gives, which is also the
  /// difference between a thump and a beep.
  ///
  /// The spacing is the design, so it reads down a column like the riffle. Even
  /// rather than hurried, and the strikes fall off a little, which is how a
  /// hand actually does it.
  knock: {
    // Lands about where a single thud does, which is the family it belongs to.
    // It can afford that where the thud cannot afford more, because a thud is
    // one of nine landing together and this is one event on its own.
    gain: 0.22,
    // One event, unlike the thud, which is one per column of a board settling.
    // Two of these overlapping is two people at the door.
    voiceCap: 2,
    layers: [
      [0.0, 1],
      [0.2, 0.95],
      [0.4, 0.86],
    ].flatMap(([delay, level]) => [
      {
        // The body. Low enough to be a door and not a drum, and the longest
        // of the three, so what is left at the end is the fundamental alone.
        source: 'sine',
        note: 96,
        sweep: { to: 62, time: 0.06 },
        env: { attack: 0.004, decay: 0.17 },
        delay,
        gain: level,
        jitter: { frequency: 0.07, gain: 0.12 },
      },
      {
        source: 'sine',
        note: 148,
        sweep: { to: 104, time: 0.05 },
        env: { attack: 0.003, decay: 0.1 },
        delay,
        gain: level * 0.5,
        jitter: { frequency: 0.08, gain: 0.15 },
      },
      {
        // The top partial, and the one with a filter closing under it: a
        // lowpass sweeping below a sine is a second decay, which is what a
        // damped panel does to its higher modes. They go first, and that is
        // most of why this reads as damped wood rather than as a tom.
        source: 'sine',
        note: 232,
        sweep: { to: 170, time: 0.045 },
        filters: [{ type: 'lowpass', frequency: 900, q: 0.7, sweep: { to: 120, time: 0.07 } }],
        env: { attack: 0.002, decay: 0.08 },
        delay,
        gain: level * 0.34,
        jitter: { frequency: 0.09, gain: 0.15 },
      },
      {
        // The knuckle landing. Rolled off hard at both ends: everything that
        // would make this a click lives above 1kHz, and the body below it is
        // the partials' business.
        source: 'noise',
        filters: [
          { type: 'lowpass', frequency: 1100, q: 0.7, sweep: { to: 240, time: 0.045 } },
          { type: 'highpass', frequency: 120, q: 0.5 },
        ],
        env: { attack: 0.002, decay: 0.05 },
        delay,
        gain: level * 0.38,
        jitter: { frequency: 0.2, gain: 0.25 },
      },
    ]),
  },

  /// Four tubes knocking together, each ringing on for seconds.
  ///
  /// Partials at 1 : 2.756 : 5.404, the free-free bar modes. Not whole
  /// multiples of anything, so there is no harmonic series and no firm pitch,
  /// just metal: a tidy octave and fifth would be a church bell and a plain
  /// sine a flute. Upper modes die far faster than the fundamental, hence
  /// three decays rather than one filter sweep.
  ///
  /// Pentatonic so any order of tubes is consonant, and struck at uneven
  /// gaps, because wind does not play scales.
  ///
  /// Its own sound rather than the board's `chime`, which means a chain
  /// paying out and is built to be gone inside a second.
  /// The three modes of a strike do not start together, and that is not a
  /// detail. Written with the same instant and the same near-zero attack they
  /// summed coherently, and twelve sines across four overlapping strikes spiked
  /// to 0.78 against a limiter at 0.841 while the sound's own average sat at a
  /// twentieth of that. A few milliseconds apart, with the fundamental slowest
  /// to arrive, the peak comes apart and the level can be set by how loud the
  /// thing is rather than by its worst instant. It is also true of real metal:
  /// the high modes speak first.
  /// The strikes are spread wide for the same reason. Tight together, four
  /// tubes each holding a three second note are a chord, twelve sines deep,
  /// and a chord's peak is however squarely its parts happen to line up. Spread
  /// out, each tube has the air mostly to itself, which is both quieter and
  /// what a wind chime is: knocks at no particular interval, not a strum.
  windChime: {
    // Set by how loud it is, which is the point of the two notes above: with
    // the strikes spread, the worst instant is about 1.6 times the nominal
    // level rather than nearly 7, so this number means something. At 0.05 it
    // was the quietest thing in the bank by a distance.
    gain: 0.15,
    voiceCap: 3,
    layers: [
      // (strike, fundamental, level)
      [0.0, 1174.7, 1],
      [0.42, 880.0, 0.85],
      [0.95, 1318.5, 0.7],
      [1.55, 1046.5, 0.9],
    ].flatMap(([delay, hz, level]) => [
      {
        // The hum, and what is still there three seconds later.
        source: 'sine',
        note: hz,
        env: { attack: 0.009, decay: 3.4 },
        delay: delay + 0.005,
        gain: level,
        jitter: { frequency: 0.004, gain: 0.15 },
      },
      {
        source: 'sine',
        note: hz * 2.756,
        env: { attack: 0.004, decay: 1.3 },
        delay: delay + 0.002,
        gain: level * 0.34,
        jitter: { frequency: 0.006, gain: 0.2 },
      },
      {
        source: 'sine',
        note: hz * 5.404,
        env: { attack: 0.001, decay: 0.5 },
        delay,
        gain: level * 0.11,
        jitter: { frequency: 0.008, gain: 0.2 },
      },
      {
        // The tubes touching. Almost nothing, and without it each note fades
        // up rather than being struck.
        source: 'noise',
        filters: [{ type: 'highpass', frequency: 5000, q: 0.7 }],
        env: { attack: 0.0008, decay: 0.02 },
        delay,
        gain: level * 0.14,
      },
    ]),
  },

  /// Air moving past, in two passes rather than one steady hiss.
  ///
  /// Written out at length rather than stretched through the `duration`
  /// option, which leaves attacks alone. This sound's attack is not a
  /// transient: it is the gust arriving, and it has to grow with the rest.
  wind: {
    gain: 0.3,
    voiceCap: 2,
    duration: 6.5,
    layers: [
      {
        source: 'noise',
        filters: [
          { type: 'bandpass', frequency: 420, q: 0.9, sweep: { to: 1500, time: 2.5 } },
          { type: 'lowpass', frequency: 2600, q: 0.5 },
        ],
        env: { attack: 1.5, hold: 0.6, decay: 4.25 },
        gain: 1,
        jitter: { frequency: 0.2, gain: 0.15 },
      },
      {
        // The second pass, narrower and darkening. A gust that rises and falls
        // once reads as a sample; two overlapping passes read as weather.
        source: 'noise',
        filters: [{ type: 'bandpass', frequency: 900, q: 2.2, sweep: { to: 300, time: 4.5 } }],
        env: { attack: 1.1, hold: 1, decay: 3 },
        delay: 0.9,
        gain: 0.5,
        jitter: { frequency: 0.25 },
      },
    ],
  },

  /// Up the slide and off the end: a foot going out from under somebody.
  /// Played by **Banana Peel**.
  ///
  /// Fast, because a slip is fast. Two octaves in 0.22s.
  whistleSlide: {
    gain: 0.24,
    voiceCap: 3,
    duration: 0.275,
    layers: [
      {
        source: 'sine',
        note: 520,
        sweep: { to: 1900, time: 0.22 },
        env: { attack: 0.012, hold: 0.15, decay: 0.06 },
        gain: 1,
        jitter: { frequency: 0.04 },
      },
      {
        // The breath, which is most of what tells a slide whistle from a sine
        // wave going up.
        source: 'noise',
        filters: [{ type: 'bandpass', frequency: 900, q: 1.4, sweep: { to: 2600, time: 0.22 } }],
        env: { attack: 0.015, hold: 0.14, decay: 0.07 },
        gain: 0.2,
      },
      {
        source: 'triangle',
        note: 520,
        detune: 1200,
        sweep: { to: 1900, time: 0.22 },
        env: { attack: 0.012, hold: 0.15, decay: 0.05 },
        gain: 0.1,
      },
    ],
  },

  /// The neck pinched, letting it down slowly.
  balloon: {
    gain: 0.2,
    voiceCap: 2,
    duration: 1.1,
    layers: [
      {
        source: 'sawtooth',
        note: 1250,
        sweep: { to: 380, time: 0.95 },
        waver: { depth: 0.1, rate: 9 },
        filters: [
          { type: 'bandpass', frequency: 1500, q: 2.6 },
          { type: 'lowpass', frequency: 3400, q: 0.6 },
        ],
        env: { attack: 0.03, hold: 0.55, decay: 0.4 },
        gain: 1,
        jitter: { frequency: 0.06 },
      },
      {
        // The air itself, which is what the squeal is riding on.
        source: 'noise',
        filters: [{ type: 'bandpass', frequency: 2200, q: 1.2, sweep: { to: 800, time: 0.9 } }],
        env: { attack: 0.04, hold: 0.5, decay: 0.42 },
        gain: 0.3,
      },
    ],
  },

  /// Three chirps, each a burst of pulses rather than a tone.
  ///
  /// A cricket rubs one wing over the other, so a chirp is a handful of
  /// strikes close enough together to read as one sound with a pitch. Written
  /// as a table because that is what it is: the numbers are the whole design.
  ///
  /// One burst of three chirps. **Cricket Chirp** plays the burst three times
  /// over with half a second of silence between, because what makes a cricket
  /// a cricket is as much the waiting as the chirping.
  cricket: {
    gain: 0.16,
    // Three plays per item, so one item fills three of these; six leaves room
    // for a second arriving on top, which a multiworld can do.
    voiceCap: 6,
    layers: [0, 0.16, 0.32].flatMap((chirp, which) =>
      [0, 0.013, 0.026, 0.039].map((pulse) => ({
        source: 'square',
        note: 4400,
        filters: [
          { type: 'bandpass', frequency: 4400, q: 7 },
          { type: 'highpass', frequency: 2600, q: 0.7 },
        ],
        env: { attack: 0.0015, hold: 0.004, decay: 0.008 },
        delay: chirp + pulse,
        gain: which === 2 ? 0.7 : 1,
        jitter: { frequency: 0.02, gain: 0.2 },
      })),
    ),
  },

  /// The real pair, 350 and 440 together, which is what a dial tone is.
  ///
  /// The 8ms attack is the tone switching on, which is what a line does.
  /// Stretching it would make this fade up like a synthesizer.
  dialTone: {
    gain: 0.14,
    voiceCap: 2,
    duration: 1.8,
    layers: [350, 440].map((note) => ({
      source: 'sine',
      note,
      env: { attack: 0.008, hold: 1.64, decay: 0.15 },
      gain: 1,
    })),
  },

  /// One burst of the other real pair: 480 and 620, which is a North American
  /// busy signal. Played three times over by **Busy Signal**.
  ///
  /// Half a second on, and the item leaves half a second off between bursts,
  /// which is the actual cadence rather than something chosen by ear: sixty
  /// interruptions a minute. The frequencies are not a choice either, any more
  /// than the dial tone's are, which is the whole appeal of these two. Written
  /// down wrong they are simply a different country's phone.
  ///
  /// Both pairs are deliberately in the bank. A dial tone and a busy signal
  /// are the same two sine waves apart, and what tells them apart is that one
  /// of them stops.
  busy: {
    gain: 0.14,
    // Three plays per item, so one item fills three of these; six leaves room
    // for a second arriving on top.
    voiceCap: 6,
    duration: 0.5,
    layers: [480, 620].map((note) => ({
      source: 'sine',
      note,
      // On and off sharply, because a line switches rather than swells, but
      // not instantly: a few milliseconds at each end is the difference
      // between a tone and a tone with a click on it.
      env: { attack: 0.006, hold: 0.47, decay: 0.024 },
      gain: 1,
    })),
  },

  /// Waves on a beach: a long swell rising and going out again. Played by
  /// **Surf**.
  ///
  /// No crack at the front: a transient at this level would be the loudest
  /// thing in the game, and without one dense low noise is water rather than
  /// weather.
  ///
  /// **The longest sound in the game**, fifteen seconds of envelope for ten
  /// audible. Nothing hushes it, so `voiceCap` holds it at two.
  ///
  /// **The fade is the body's own decay, not a layer of its own.** A tail
  /// layer fading in at six seconds restarted the sound at 0.40 after the
  /// others had reached 0.0007: `exponentialRampToValueAtTime(0.0001)` is an
  /// eighty decibel fall, so a layer is inaudible long before its decay
  /// nominally ends. One envelope that rises once and falls once cannot come
  /// back, whatever the numbers are.
  surf: {
    // High for this file and still one of the quietest things in it: dense
    // low noise peaks close to its average where a struck note peaks many
    // times its own.
    gain: 0.75,
    voiceCap: 2,
    duration: 15.3,
    layers: [
      {
        // The swell: four and a half seconds at full, then ten of decay, which
        // is about five seconds of audible fade before it is under everything
        // else. The level holds while the filter keeps closing, so what
        // changes during the plateau is the color rather than the loudness,
        // which is what a wave going over does.
        source: 'noise',
        filters: [{ type: 'lowpass', frequency: 220, q: 0.7, sweep: { to: 70, time: 7 } }],
        env: { attack: 0.8, hold: 4.5, decay: 10 },
        gain: 1,
        jitter: { frequency: 0.2, gain: 0.2 },
      },
      {
        // A second wave over the first, so it keeps moving rather than simply
        // fading, and high enough to survive a speaker that cannot move the
        // layer above.
        //
        // It arrives during the swell and leaves with it. Its own peak lands
        // just as the body's plateau ends, which is what keeps the sum rising
        // once and falling once: a wave that peaked after the body had gone
        // quiet would be the fault this sound just had.
        source: 'noise',
        filters: [
          { type: 'lowpass', frequency: 420, q: 0.6, sweep: { to: 120, time: 8 } },
          { type: 'highpass', frequency: 90, q: 0.5 },
        ],
        env: { attack: 1.4, hold: 2.2, decay: 7 },
        delay: 1.8,
        gain: 0.5,
        jitter: { frequency: 0.18, gain: 0.2 },
      },
    ],
  },

  /// A fart. Played by **Barking Spider**.
  ///
  /// Stick and slip rather than a note: something catches, lets go, catches
  /// again, and the pitch jumps about while it does. A deep slow `waver` over
  /// a rising glide is that, where the rocket's shallow fast one is a whistle
  /// refusing to sit still. Same mechanism as a creaking floorboard, which is
  /// what this was first written as.
  ///
  /// The item's name is the euphemism and the sound's is what it is. One of
  /// them lands in other people's feeds; the other only has to tell whoever is
  /// editing this file what they are editing.
  ///
  /// **The sweep carries no `time` on purpose.** A waver is steps along the
  /// glide, so a glide that ends early takes the wobble with it and the rest
  /// of the note sits dead still. Omitting `time` glides across the whole
  /// envelope, so nothing has to agree with anything when the envelope moves.
  fart: {
    // High for the same reason the surf is: a narrow band of sawtooth that
    // never holds a pitch has no transient to peak on.
    gain: 0.4,
    voiceCap: 2,
    duration: 1.4,
    layers: [
      {
        source: 'sawtooth',
        note: 165,
        sweep: { to: 395 },
        waver: { depth: 0.28, rate: 13 },
        filters: [
          { type: 'bandpass', frequency: 760, q: 3.4 },
          { type: 'highpass', frequency: 220, q: 0.6 },
        ],
        env: { attack: 0.03, hold: 1, decay: 0.37 },
        gain: 1,
      },
    ],
  },

  /// One burst of four. Played three times over by **Kitchen Timer**.
  ///
  /// The burst is the sound and the repeating is the item's, the same split
  /// the cricket uses: what a burst is belongs here, how insistent the thing
  /// is belongs next to the name.
  beep: {
    gain: 0.14,
    // Three plays per item; six leaves room for a second arriving on top.
    voiceCap: 6,
    layers: [0, 0.12, 0.24, 0.36].map((delay) => ({
      source: 'square',
      note: 2100,
      // Everything above the fundamental taken off, which is what makes an
      // appliance beep flat and cheap rather than a musical note.
      filters: [{ type: 'lowpass', frequency: 4200, q: 0.7 }],
      // Shorter as well as closer, so four in the time the old three took
      // still have silence between them rather than running together.
      env: { attack: 0.002, hold: 0.055, decay: 0.02 },
      delay,
      gain: 1,
    })),
  },

  // Two more voices for written figures, alongside `keys`, `glass` and
  // `bass` above. Same contract: no `note` of their own, and a `duration` the
  // sequencer stretches to the written length of each note.

  /// A muted trombone.
  ///
  /// **The "wah" is the filter, not the pitch.** A plunger opens and shuts the
  /// bell, which changes how much of the harmonic series gets out and changes
  /// the pitch not at all: a resonant lowpass climbing from nearly shut to
  /// open across the note, dark and nasal to bright, which is the vowel.
  ///
  /// A lowpass and not a bandpass. A bandpass at the formant throws away
  /// everything under it including the fundamental, thinning a low brass note
  /// to a buzz; a resonant lowpass puts a peak in the same place and keeps the
  /// body below it. Also how a real wah pedal is built.
  ///
  /// **Every note scoops up into pitch**, which is what `sweep.from` is for.
  /// A tone and a half below arriving in 75ms, so the note is in tune for all
  /// of itself except the approach.
  ///
  /// **The long note leans into a vibrato half a second in.** `after` is in
  /// absolute seconds, so the three short notes at 0.47s never reach it and
  /// one voice covers the whole figure.
  brass: {
    // The resonant lowpass peaks on the fundamental at the closed end, which
    // made this the loudest sound in the bank before the trim.
    gain: 0.23,
    duration: 0.5,
    layers: [
      {
        source: 'sawtooth',
        sweep: { from: -150, time: 0.075 },
        vibrato: { depth: 22, rate: 5.5, after: 0.5 },
        filters: [
          // The plunger coming off the bell. Q high enough to be a vowel
          // rather than a volume change, and starting below the lowest note
          // of the figure so the closed end is properly shut: the resonant
          // peak sits on the fundamental and almost nothing above it gets
          // out. These two numbers are the dial for how pronounced the wah
          // is, and nothing else here is.
          { type: 'lowpass', frequency: 220, q: 4.5, sweep: { to: 2000, time: 0.38 } },
          { type: 'highpass', frequency: 110, q: 0.6 },
        ],
        env: { attack: 0.035, hold: 0.22, decay: 0.26 },
        gain: 1,
      },
      {
        // The rasp over the top, which a sawtooth through a lowpass does not
        // have on its own. It scoops and wavers with the layer below rather
        // than holding still, or the two would drift apart and the note would
        // sound like two instruments.
        source: 'square',
        detune: 1200,
        sweep: { from: -150, time: 0.075 },
        vibrato: { depth: 22, rate: 5.5, after: 0.5 },
        filters: [{ type: 'bandpass', frequency: 900, q: 1.8, sweep: { to: 2400, time: 0.38 } }],
        env: { attack: 0.05, hold: 0.18, decay: 0.2 },
        gain: 0.08,
      },
    ],
  },

  /// A kazoo: `fart`'s recipe with the note coming from outside, since both
  /// are a membrane buzzing against moving air. One sawtooth through one
  /// narrow band at a fixed 760Hz over a gentle highpass.
  ///
  /// The fixed band does the work. A sawtooth's harmonics slide past a
  /// stationary formant as the note changes, which is a nasal honk rather
  /// than a tone, and is how a real kazoo behaves: the membrane resonates
  /// where it resonates whatever you hum at it.
  ///
  /// **The waver is shallower than the fart's on purpose.** 28% is four
  /// semitones of random walk, fine for something that is not a note; this
  /// has a tune to play, so 4% warbles audibly and leaves the melody
  /// standing. It is the dial if this wants to be fartier.
  ///
  /// The sag in pitch is partly that a kazoo sags and partly that a `waver`
  /// needs a glide to walk along. No `time` on it, for the reason `fart`
  /// gives.
  kazoo: {
    // A q of 3.4 throws away most of a sawtooth.
    gain: 0.44,
    duration: 0.32,
    layers: [
      {
        source: 'sawtooth',
        sweep: { by: -30 },
        waver: { depth: 0.04, rate: 14 },
        filters: [
          { type: 'bandpass', frequency: 760, q: 3.4 },
          // Lower than the fart's 220: this one has to let a C4 through at
          // 262, and that one never plays a note at all.
          { type: 'highpass', frequency: 180, q: 0.6 },
        ],
        env: { attack: 0.02, hold: 0.12, decay: 0.2 },
        gain: 1,
      },
    ],
  },

};

/// What each named filler item sounds like, in the engine's own order.
///
/// The engine owns the names and the numbers; this owns the sounds. An item
/// event carries an index into the engine's item table, the engine says which
/// noise sits there, and that number indexes this list. So the two orders have
/// to agree exactly, and `check_abi.py` compares them at build time: the names
/// are here only so that comparison has something to compare, and `main.js`
/// never reads one.
///
/// Getting the order wrong is the one mistake here nobody would ever report as
/// a bug. Every item would still arrive, still be named correctly in the feed,
/// and play the sound belonging to its neighbor. It would simply be slightly
/// wrong forever.
///
/// Two shapes:
///
///   plays   one or more sounds, each `{ sound, ...options }` as `Audio.play`
///           takes them, so a noise can be an existing sound detuned, or the
///           same sound twice a moment apart
///   figure  `{ tempo, parts }` for `Audio.sequence`: a written tune, for the
///           handful of these that are tunes
export const NOISES = [
  // Some of these are built from sounds the game already had, shifted well
  // away from the pitch the board plays them at: a filler item that sounds
  // like a refused swap reads as the board talking rather than as an item.
  { name: 'Door Knock', plays: [{ sound: 'knock' }] },
  {
    name: 'Busy Signal',
    // Half a second on, half off, three times. The starts are exactly a
    // second apart because that is the real cadence, sixty interruptions a
    // minute, rather than a gap picked by ear like the cricket's.
    plays: [
      { sound: 'busy' },
      { sound: 'busy', delay: 1 },
      { sound: 'busy', delay: 2 },
    ],
  },
  {
    name: 'Wind Chime',
    plays: [{ sound: 'windChime' }],
  },
  // Two octaves up, where the bell's long decay reads as small and bright
  // rather than as a church.
  { name: 'Tiny Bell', plays: [{ sound: 'bell', detune: 2400, gain: 0.8 }] },
  {
    name: 'Sour Note',
    // Three notes a semitone or so apart, held, low. The register is what
    // makes it rough rather than merely dissonant: at G#3 a semitone is 12Hz
    // against a critical band nearer 90, so the pair falls inside one band.
    // Two octaves up the same notes would just be a minor second.
    //
    // The third sits 42 cents under the top one, out of tune with both. One
    // wrong note is a mistake; three that disagree about the mistake is sour.
    //
    // Rolled a couple of hundredths apart, like a fumbled chord, and each
    // voice well down: three sustained notes at full level would be the
    // loudest thing in the game.
    //
    // 2.6 and not 2 because the decay is exponential, so the last half second
    // is under -48dB. This is two seconds of *audible* note.
    plays: [
      { sound: 'keys', note: 'G#3', duration: 2.6, gain: 0.4 },
      { sound: 'keys', note: 'A3', duration: 2.6, delay: 0.02, gain: 0.36 },
      { sound: 'keys', note: 'A3', detune: -42, duration: 2.6, delay: 0.035, gain: 0.26 },
    ],
  },

  { name: 'Gust of Wind', plays: [{ sound: 'wind' }] },
  { name: 'Banana Peel', plays: [{ sound: 'whistleSlide' }] },
  { name: 'Deflating Balloon', plays: [{ sound: 'balloon' }] },
  {
    name: 'Cricket Chirp',
    // The burst three times over, half a second of silence between each. The
    // burst runs 0.37s, so the starts are 0.87 apart rather than 0.5: a gap
    // means a gap, and spacing the starts half a second apart would leave a
    // tenth of a second of silence and read as one long stream of chirping.
    plays: [
      { sound: 'cricket' },
      { sound: 'cricket', delay: 0.87 },
      { sound: 'cricket', delay: 1.74 },
    ],
  },
  { name: 'Dial Tone', plays: [{ sound: 'dialTone' }] },
  { name: 'Surf', plays: [{ sound: 'surf' }] },
  { name: 'Barking Spider', plays: [{ sound: 'fart' }] },
  {
    name: 'Kitchen Timer',
    // Four beeps, then half a second, three times over. The burst runs 0.44s,
    // so the starts are 0.94 apart to leave the gap actually silent.
    plays: [
      { sound: 'beep' },
      { sound: 'beep', delay: 0.94 },
      { sound: 'beep', delay: 1.88 },
    ],
  },

  // The five that are tunes.
  {
    name: 'Sad Trombone',
    // Four notes down, each scooped up into from below and each wah-ing as the
    // plunger comes off. All of that is the voice's, not the figure's: see
    // `brass`.
    //
    // The three short notes ring three quarters of a beat and then stop,
    // leaving an eighth of a second of silence before the next. Detached and
    // not slurred, which is how the joke goes: wah, wah, wah, waaaah. It is
    // also what keeps them under the half second where `brass` starts its
    // vibrato, so only the last note leans.
    //
    // The last note is three beats, up from 2.2. Nearly two seconds, most of
    // which is the vibrato, because the length is the punchline.
    figure: {
      tempo: 96,
      parts: [
        {
          sound: 'brass',
          notes: [
            ['Bb3', 0, 0.75],
            ['A3', 1, 0.75],
            ['Ab3', 2, 0.75],
            ['G3', 3, 3],
          ],
        },
      ],
    },
  },
  {
    name: 'Shave and a Haircut',
    // On the melody voice the victory tune uses, with its bassline underneath.
    // Both are already balanced against each other, which is most of why they
    // are reused here rather than given a voice of their own.
    figure: {
      tempo: 152,
      parts: [
        {
          sound: 'keys',
          notes: [
            ['C5', 0, 0.5],
            ['G4', 0.5, 0.25],
            ['G4', 0.75, 0.25],
            ['A4', 1, 0.5],
            ['G4', 1.5, 0.5],
            ['B4', 2.5, 0.5],
            ['C5', 3, 0.75],
          ],
          pan: -0.1,
        },
        {
          sound: 'bass',
          notes: [
            ['C3', 0, 1],
            ['G2', 1.5, 1],
            ['C3', 3, 1.25],
          ],
        },
      ],
    },
  },
  {
    name: 'Triumphant Kazoo',
    figure: {
      tempo: 144,
      parts: [
        {
          sound: 'kazoo',
          notes: [
            ['C4', 0, 0.5],
            ['E4', 0.5, 0.5],
            ['G4', 1, 0.5],
            ['C5', 1.5, 1.4],
          ],
        },
      ],
    },
  },
];
