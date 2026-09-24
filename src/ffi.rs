//! The C ABI the browser front end calls.
//!
//! Deliberately hand-written rather than generated: the engine has no
//! dependencies, so the wasm build is a plain `cargo build --target
//! wasm32-unknown-unknown` with no bindgen step and nothing to fetch.
//!
//! The boundary is kept cheap on purpose. Per-frame data (the whole board and
//! its animation offsets) is written into two buffers the front end reads
//! straight out of wasm memory, so drawing a frame costs one call and two
//! typed-array views rather than a call per cell.
//!
//! # Safety
//!
//! Every function here takes the pointer returned by [`tg_create`]. Passing
//! anything else, or using a pointer after [`tg_destroy`], is undefined
//! behavior. Null is tolerated and returns a neutral value so a front-end bug
//! cannot trap the module.

use crate::board::{Pos, Special};
use crate::game::{Event, Status, Tap};
use crate::session::Session;

/// Bytes per packed event; mirrored by the front end's event reader.
pub const EVENT_SIZE: usize = 8;

/// Opaque to the front end: a session pointer and nothing more.
pub struct Handle {
    session: Session,
    events: Vec<u8>,
    /// Scratch for [`tg_checked_ptr`], which has to hand out a pointer to
    /// something that outlives the call.
    checked: Vec<u8>,
}

/// Creates a session. The seed is passed as two halves because wasm's JS
/// boundary has no unsigned 64-bit type.
///
/// # Safety
/// The returned pointer must eventually be freed with [`tg_destroy`].
#[no_mangle]
pub extern "C" fn tg_create(seed_lo: u32, seed_hi: u32) -> *mut Handle {
    let seed = ((seed_hi as u64) << 32) | seed_lo as u64;
    let handle =
        Box::new(Handle { session: Session::new(seed), events: Vec::new(), checked: Vec::new() });
    Box::into_raw(handle)
}

/// # Safety
/// `handle` must come from [`tg_create`] and must not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn tg_destroy(handle: *mut Handle) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Reads a handle, or returns `$default` when the front end passed null.
macro_rules! session {
    ($handle:expr, $default:expr) => {
        match unsafe { $handle.as_ref() } {
            Some(handle) => handle,
            None => return $default,
        }
    };
}

macro_rules! session_mut {
    ($handle:expr, $default:expr) => {
        match unsafe { $handle.as_mut() } {
            Some(handle) => handle,
            None => return $default,
        }
    };
}

// ---- the clock and input -------------------------------------------------

/// Advances the game and refreshes the snapshot and event buffers.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_update(handle: *mut Handle, dt_ms: f32) {
    let handle = session_mut!(handle, ());
    handle.session.update(dt_ms);
    pack_events(handle);
}

/// Taps a cell. Returns 0 ignored, 1 selected, 2 deselected, 3 swap started.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_tap(handle: *mut Handle, r: i32, c: i32) -> u32 {
    let handle = session_mut!(handle, 0);
    let outcome = handle.session.game_mut().tap(Pos::new(r, c));
    pack_events(handle);
    match outcome {
        Tap::Ignored => 0,
        Tap::Selected => 1,
        Tap::Deselected => 2,
        Tap::Swapped => 3,
    }
}

/// Starts a swap between two cells, for drag and swipe input. Returns 1 when
/// the swap was accepted.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_swap(
    handle: *mut Handle,
    r1: i32,
    c1: i32,
    r2: i32,
    c2: i32,
) -> u32 {
    let handle = session_mut!(handle, 0);
    let accepted = handle.session.game_mut().try_swap(Pos::new(r1, c1), Pos::new(r2, c2));
    pack_events(handle);
    accepted as u32
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_clear_selection(handle: *mut Handle) {
    let handle = session_mut!(handle, ());
    handle.session.game_mut().clear_selection();
}

// ---- level navigation ----------------------------------------------------

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_load_level(handle: *mut Handle, index: u32) -> u32 {
    let handle = session_mut!(handle, 0);
    handle.session.load(index as usize) as u32
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_next_level(handle: *mut Handle) -> u32 {
    let handle = session_mut!(handle, 0);
    handle.session.next_level() as u32
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_retry(handle: *mut Handle) {
    let handle = session_mut!(handle, ());
    handle.session.retry();
}

/// Restores a returning player's progress, as read from browser storage.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_set_unlocked(handle: *mut Handle, count: u32) {
    let handle = session_mut!(handle, ());
    handle.session.set_unlocked(count as usize);
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_count(handle: *const Handle) -> u32 {
    session!(handle, 0).session.level_count() as u32
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_index(handle: *const Handle) -> u32 {
    session!(handle, 0).session.index() as u32
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_unlocked(handle: *const Handle) -> u32 {
    session!(handle, 0).session.unlocked() as u32
}

/// UTF-8 bytes of the current level's name; read `tg_level_name_len` of them.
///
/// # Safety
/// `handle` must come from [`tg_create`]. The pointer is valid until the next
/// level load.
#[no_mangle]
pub unsafe extern "C" fn tg_level_name_ptr(handle: *const Handle) -> *const u8 {
    session!(handle, std::ptr::null()).session.level_name().as_ptr()
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_name_len(handle: *const Handle) -> u32 {
    session!(handle, 0).session.level_name().len() as u32
}

/// Every level name, newline separated; read `tg_level_names_len` bytes.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_names_ptr(handle: *const Handle) -> *const u8 {
    session!(handle, std::ptr::null()).session.level_names().as_ptr()
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_names_len(handle: *const Handle) -> u32 {
    session!(handle, 0).session.level_names().len() as u32
}

/// Every item's name, newline separated, in the order the engine numbers them.
/// An item event carries a number into this list rather than any text.
///
/// The engine owns these because they are the same strings a tracker and a
/// spoiler log will show: a front end building its own would drift the first
/// time either side was edited.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_item_names_ptr(handle: *const Handle) -> *const u8 {
    session!(handle, std::ptr::null()).session.item_names().as_ptr()
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_item_names_len(handle: *const Handle) -> u32 {
    session!(handle, 0).session.item_names().len() as u32
}

/// Every location's name, the same way. See [`tg_item_names_ptr`].
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_location_names_ptr(handle: *const Handle) -> *const u8 {
    session!(handle, std::ptr::null()).session.location_names().as_ptr()
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_location_names_len(handle: *const Handle) -> u32 {
    session!(handle, 0).session.location_names().len() as u32
}

// ---- how the run is set up -----------------------------------------------

/// The settings table as text: one line per setting, tab separated fields.
///
/// The front end builds its options screen by walking this rather than by
/// knowing what the settings are, so a setting added to the engine appears on
/// the screen, in the yaml and in the generated apworld together. See
/// [`crate::session::Session::option_table`] for the fields.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_options_ptr(handle: *const Handle) -> *const u8 {
    session!(handle, std::ptr::null()).session.option_table().as_ptr()
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_options_len(handle: *const Handle) -> u32 {
    session!(handle, 0).session.option_table().len() as u32
}

/// What one setting is set to. `u32::MAX` for a setting that does not exist.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_option_value(handle: *const Handle, at: u32) -> u32 {
    session!(handle, u32::MAX).session.options().get(at as usize).unwrap_or(u32::MAX)
}

/// The value one step along, which the engine works out because a range stops
/// at its ends and a choice goes round.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_option_step(handle: *const Handle, at: u32, by: i32) -> u32 {
    session!(handle, 0).session.step_option(at as usize, by)
}

/// Sets one setting, which starts the run over on the same seed. Returns 1 if
/// it took, 0 for a value the setting does not allow.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_set_option(handle: *mut Handle, at: u32, value: u32) -> u32 {
    let Some(handle) = handle.as_mut() else { return 0 };
    u32::from(handle.session.set_option(at as usize, value))
}

// ---- reading the board ---------------------------------------------------

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_rows(handle: *const Handle) -> u32 {
    session!(handle, 0).session.game().board.rows as u32
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_cols(handle: *const Handle) -> u32 {
    session!(handle, 0).session.game().board.cols as u32
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_colors(handle: *const Handle) -> u32 {
    session!(handle, 0).session.game().rules().colors as u32
}

/// Four bytes per cell: color (255 when empty), special, jelly layers, flags.
///
/// # Safety
/// `handle` must come from [`tg_create`]. The pointer is valid until the next
/// call that mutates the session, so re-read it each frame.
#[no_mangle]
pub unsafe extern "C" fn tg_cells_ptr(handle: *const Handle) -> *const u8 {
    session!(handle, std::ptr::null()).session.game().cells_bytes().as_ptr()
}

/// Three floats per cell: x offset, y offset (in cell widths), scale.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_offsets_ptr(handle: *const Handle) -> *const f32 {
    session!(handle, std::ptr::null()).session.game().offsets().as_ptr()
}

// ---- the state a HUD shows -----------------------------------------------

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_score(handle: *const Handle) -> f64 {
    session!(handle, 0.0).session.game().progress.score as f64
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_moves_left(handle: *const Handle) -> u32 {
    session!(handle, 0).session.game().moves_left
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_moves_total(handle: *const Handle) -> u32 {
    session!(handle, 0).session.game().spec.moves
}

/// 0 idle, 1 swapping, 2 clearing, 3 falling, 4 shuffling, 5 finished.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_phase(handle: *const Handle) -> u32 {
    session!(handle, 0).session.game().phase().code()
}

/// 0 playing, 1 won, 2 lost.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_status(handle: *const Handle) -> u32 {
    match session!(handle, 0).session.game().status() {
        Status::Playing => 0,
        Status::Won => 1,
        Status::Lost => 2,
    }
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_cascade(handle: *const Handle) -> u32 {
    session!(handle, 0).session.game().cascade()
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_accepts_input(handle: *const Handle) -> u32 {
    session!(handle, 0).session.game().accepts_input() as u32
}

/// Which specials the run may make, as a bitmask indexed by [`Special`]'s own
/// codes: bit 1 is `LineH`, bit 5 is `Rocket`. Bit 0 is never set.
///
/// A mask rather than a call per special, because the front end wants the
/// whole set at once and this way a sixth special costs nothing at the
/// boundary.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_unlocked_specials(handle: *const Handle) -> u32 {
    let set = session!(handle, 0).session.inventory().specials();
    let held = [
        (set.line_h, Special::LineH),
        (set.line_v, Special::LineV),
        (set.cross, Special::Cross),
        (set.rainbow, Special::Rainbow),
        (set.rocket, Special::Rocket),
    ];
    held.iter().filter(|(on, _)| *on).map(|(_, s)| 1u32 << s.code()).sum()
}

/// The current level's silver and gold marks, or 0 when it offers neither.
///
/// Scores worth coming back for rather than goals: the level ends on its
/// objectives whatever the score. Two calls rather than one packed value
/// because a score does not fit in half a `u32`.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_silver(handle: *const Handle) -> f64 {
    session!(handle, 0.0).session.level().silver as f64
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_gold(handle: *const Handle) -> f64 {
    session!(handle, 0.0).session.level().gold as f64
}

/// How well the level at `index` has been beaten, at best: 0 never, 1 cleared,
/// 2 past silver, 3 past gold. See [`Tier`].
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_best(handle: *const Handle, index: u32) -> u32 {
    session!(handle, 0).session.best_tier(index as usize).code()
}

/// Which locations this run has already checked, as little-endian `u32` ids.
///
/// Written into the save, so a returning run keeps what it found and stays
/// unable to find it twice. The pointer is only good until the next call.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_checked_ptr(handle: *mut Handle) -> *const u8 {
    let handle = session_mut!(handle, std::ptr::null());
    handle.checked.clear();
    for id in handle.session.checked() {
        handle.checked.extend_from_slice(&id.to_le_bytes());
    }
    handle.checked.as_ptr()
}

/// How many ids [`tg_checked_ptr`] has to offer.
///
/// Read from the run rather than from the buffer that call fills, so the two
/// can be asked in either order.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_checked_len(handle: *const Handle) -> u32 {
    session!(handle, 0).session.checked().len() as u32
}

/// Hands back a location the run had checked before, rebuilding what it was
/// worth. Quiet: restoring a save should not replay every item through the
/// feed. Ids nothing recognizes are ignored.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_restore(handle: *mut Handle, id: u32) {
    let handle = session_mut!(handle, ());
    handle.session.restore(id);
    // Packs like every other call that changes something, so a caller can see
    // that restoring raised nothing rather than having to take it on trust.
    pack_events(handle);
}

/// A legal move packed as `r1 << 24 | c1 << 16 | r2 << 8 | c2`, or `u32::MAX`
/// when the board has none.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_hint(handle: *const Handle) -> u32 {
    match session!(handle, u32::MAX).session.game().hint() {
        Some((a, b)) => {
            ((a.r as u32) << 24) | ((a.c as u32) << 16) | ((b.r as u32) << 8) | b.c as u32
        }
        None => u32::MAX,
    }
}

// ---- objectives ----------------------------------------------------------

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_objective_count(handle: *const Handle) -> u32 {
    session!(handle, 0).session.game().objectives().len() as u32
}

/// 0 score, 1 color, 2 jelly.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_objective_kind(handle: *const Handle, index: u32) -> u32 {
    let game = session!(handle, 0).session.game();
    game.objectives().get(index as usize).map_or(0, |o| o.kind_code())
}

/// The color an objective concerns, or 255.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_objective_color(handle: *const Handle, index: u32) -> u32 {
    let game = session!(handle, 255).session.game();
    game.objectives().get(index as usize).map_or(255, |o| o.color() as u32)
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_objective_need(handle: *const Handle, index: u32) -> u32 {
    let game = session!(handle, 0).session.game();
    game.objectives().get(index as usize).map_or(0, |o| o.needed(&game.progress))
}

/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_objective_have(handle: *const Handle, index: u32) -> u32 {
    let game = session!(handle, 0).session.game();
    game.objectives().get(index as usize).map_or(0, |o| o.reached(&game.progress))
}

// ---- events --------------------------------------------------------------

/// Number of events raised by the last call; each is [`EVENT_SIZE`] bytes.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_events_len(handle: *const Handle) -> u32 {
    (session!(handle, 0).events.len() / EVENT_SIZE) as u32
}

/// Packed events: kind, row, col, color, special, cascade, then a little-endian
/// `u16` value. Rows and columns are 255 for events that are not about a cell.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_events_ptr(handle: *const Handle) -> *const u8 {
    session!(handle, std::ptr::null()).events.as_ptr()
}

fn pack_events(handle: &mut Handle) {
    handle.events.clear();
    // The board's own, then the run's. One stream: the page should not have to
    // ask two places what just happened.
    let mut events: Vec<Event> = handle.session.game().events().to_vec();
    events.extend_from_slice(handle.session.events());
    for event in events {
        handle.events.extend_from_slice(&[
            event.kind,
            event.r,
            event.c,
            event.color,
            event.special,
            event.cascade,
            event.value as u8,
            (event.value >> 8) as u8,
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::EV_SWAP;
    use crate::options::SETTINGS;

    /// Drives the ABI the way the front end does, to catch a mismatch between
    /// what the engine knows and what it is willing to say.
    #[test]
    fn the_abi_round_trips_a_move() {
        unsafe {
            let handle = tg_create(1234, 0);
            assert!(!handle.is_null());
            // Against the level's own rules rather than a number written here
            // twice: what this checks is that the ABI reports the board the
            // engine actually has, not that the board is any given size.
            let spec = &crate::level::levels()[0];
            assert_eq!(tg_rows(handle), spec.rules.rows as u32);
            assert_eq!(tg_cols(handle), spec.rules.cols as u32);
            assert_eq!(tg_level_index(handle), 0);
            assert_eq!(tg_status(handle), 0);
            assert_eq!(tg_accepts_input(handle), 1);
            assert_eq!(tg_moves_left(handle), tg_moves_total(handle));

            let name_len = tg_level_name_len(handle) as usize;
            let name = std::slice::from_raw_parts(tg_level_name_ptr(handle), name_len);
            assert_eq!(name, b"First Light");

            let hint = tg_hint(handle);
            assert_ne!(hint, u32::MAX, "a fresh board always has a move");
            let (r1, c1, r2, c2) = (
                (hint >> 24) as i32,
                ((hint >> 16) & 0xff) as i32,
                ((hint >> 8) & 0xff) as i32,
                (hint & 0xff) as i32,
            );
            assert_eq!(tg_swap(handle, r1, c1, r2, c2), 1);
            assert_eq!(tg_accepts_input(handle), 0, "the board is busy mid-swap");

            let events = std::slice::from_raw_parts(
                tg_events_ptr(handle),
                tg_events_len(handle) as usize * EVENT_SIZE,
            );
            assert_eq!(events[0], EV_SWAP);

            for _ in 0..400 {
                tg_update(handle, 16.0);
            }
            assert!(tg_score(handle) > 0.0, "the hinted move should have scored");
            assert_eq!(tg_phase(handle), 0, "the board should be idle again");

            tg_destroy(handle);
        }
    }

    #[test]
    fn the_snapshot_buffers_cover_the_board() {
        unsafe {
            let handle = tg_create(5, 0);
            let count = (tg_rows(handle) * tg_cols(handle)) as usize;
            let cells = std::slice::from_raw_parts(tg_cells_ptr(handle), count * 4);
            let offsets = std::slice::from_raw_parts(tg_offsets_ptr(handle), count * 3);
            // 255 is the empty cell, which a walled layout has plenty of.
            // Every other byte names a color the level actually deals, which
            // is not the same as one below the count: a level may name any
            // four of the eight rather than the first four.
            let level = &crate::level::levels()[0];
            assert!(
                cells.chunks(4).all(|c| c[0] == 255 || level.rules.deals(c[0])),
                "a gem on the board is a color the level never deals",
            );
            assert!(cells.chunks(4).any(|c| c[0] != 255), "the board is empty");
            assert!(offsets.chunks(3).all(|o| o[2] == 1.0));
            tg_destroy(handle);
        }
    }

    #[test]
    fn objectives_are_readable_and_start_at_zero() {
        unsafe {
            let handle = tg_create(5, 0);
            // Read the ladder rather than pinning level one's shape here, so
            // redesigning a level does not break the ABI's test. What is being
            // checked is that every objective comes across whole.
            let level = &crate::level::levels()[0];
            assert_eq!(tg_objective_count(handle) as usize, level.objectives.len());
            for (at, objective) in level.objectives.iter().enumerate() {
                let at = at as u32;
                assert_eq!(tg_objective_kind(handle, at), objective.kind_code());
                assert_eq!(tg_objective_color(handle, at), objective.color() as u32);
                assert!(tg_objective_need(handle, at) > 0, "an objective asks for nothing");
                assert_eq!(tg_objective_have(handle, at), 0, "a level opens part done");
            }
            // Reading past the end is answered, not trapped.
            assert_eq!(tg_objective_need(handle, 99), 0);
            tg_destroy(handle);
        }
    }

    #[test]
    fn locked_levels_stay_locked_across_the_boundary() {
        unsafe {
            let handle = tg_create(5, 0);
            assert_eq!(tg_unlocked(handle), 1);
            assert_eq!(tg_load_level(handle, 4), 0);
            assert_eq!(tg_level_index(handle), 0);
            assert_eq!(tg_load_level(handle, 0), 1);
            tg_destroy(handle);
        }
    }

    #[test]
    fn restored_progress_opens_the_level_select() {
        unsafe {
            let handle = tg_create(5, 0);
            assert_eq!(tg_load_level(handle, 3), 0);
            tg_set_unlocked(handle, 5);
            assert_eq!(tg_unlocked(handle), 5);
            assert_eq!(tg_load_level(handle, 3), 1);
            assert_eq!(tg_level_index(handle), 3);
            tg_destroy(handle);
        }
    }

    #[test]
    fn the_level_picker_can_read_every_name() {
        unsafe {
            let handle = tg_create(5, 0);
            let bytes = std::slice::from_raw_parts(
                tg_level_names_ptr(handle),
                tg_level_names_len(handle) as usize,
            );
            let names: Vec<&str> = std::str::from_utf8(bytes).unwrap().split('\n').collect();
            assert_eq!(names.len(), tg_level_count(handle) as usize);
            assert_eq!(names[0], "First Light");
            tg_destroy(handle);
        }
    }

    #[test]
    fn the_options_screen_can_read_the_whole_table() {
        // The front end builds its controls out of this text and nothing
        // else, so every setting has to come through it with enough to draw
        // one: what it is called, what it does, and either two bounds or a
        // list of labeled values.
        unsafe {
            let handle = tg_create(5, 0);
            let bytes = std::slice::from_raw_parts(
                tg_options_ptr(handle),
                tg_options_len(handle) as usize,
            );
            let lines: Vec<&str> = std::str::from_utf8(bytes).unwrap().split('\n').collect();
            assert_eq!(lines.len(), SETTINGS.len());
            for (at, line) in lines.iter().enumerate() {
                let fields: Vec<&str> = line.split('\t').collect();
                assert_eq!(fields[0], SETTINGS[at].key);
                assert!(!fields[1].is_empty(), "{} has no label", fields[0]);
                assert!(!fields[2].is_empty(), "{} says nothing about itself", fields[0]);
                match fields[3] {
                    "range" => assert_eq!(fields.len(), 7, "a range wants two bounds"),
                    "choice" => assert!(fields.len() > 5, "a choice wants values"),
                    other => panic!("{} is a {other}, which the screen cannot draw", fields[0]),
                }
                assert_eq!(fields[4].parse::<u32>().unwrap(), SETTINGS[at].default);
            }
            tg_destroy(handle);
        }
    }

    #[test]
    fn setting_an_option_deals_the_run_again_and_a_bad_value_does_not() {
        unsafe {
            let handle = tg_create(5, 0);
            let goal = 0;
            let was = tg_option_value(handle, goal);
            let next = tg_option_step(handle, goal, 1);
            assert_ne!(next, was, "stepping a setting did not move it");

            assert_eq!(tg_set_option(handle, goal, next), 1);
            assert_eq!(tg_option_value(handle, goal), next);
            // Back to the first level, because a setting changes what the run
            // is rather than what is happening in it.
            assert_eq!(tg_level_index(handle), 0);
            assert_eq!(tg_unlocked(handle), 1);

            assert_eq!(tg_set_option(handle, goal, 9_999), 0, "a value no setting allows took");
            assert_eq!(tg_option_value(handle, goal), next, "and it changed things anyway");
            assert_eq!(tg_option_value(handle, 99), u32::MAX);
            tg_destroy(handle);
        }
    }

    #[test]
    fn a_null_handle_is_answered_rather_than_trapped() {
        unsafe {
            let null: *mut Handle = std::ptr::null_mut();
            assert_eq!(tg_rows(null), 0);
            assert_eq!(tg_hint(null), u32::MAX);
            assert_eq!(tg_score(null), 0.0);
            assert_eq!(tg_tap(null, 0, 0), 0);
            assert!(tg_cells_ptr(null).is_null());
            tg_update(null, 16.0);
            tg_set_unlocked(null, 3);
            tg_destroy(null);
        }
    }
}
