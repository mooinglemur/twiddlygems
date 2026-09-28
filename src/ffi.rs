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
use crate::progression::{plays_generator, Consumable, Item, AP_ID_BASE, GENERATOR, UNLOCKS};
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

/// How much the world cares about the item at `index` in that same list: 0
/// filler, 1 useful, 2 progression, 3 trap. See `Class`.
///
/// The page colors an item's name in the feed by this, the way an Archipelago
/// client does, and it is the same answer that goes into the world's data as
/// the item's classification. An index past the end reads as filler, which is
/// the answer that claims the least.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_item_class(handle: *const Handle, index: u32) -> u32 {
    session!(handle, 0).session.item_class(index as usize).code()
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

/// How many of the Archipelago gems on the level at `index` this run has
/// taken. Against [`tg_gems_per_level`], which is how many there are.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_gems(handle: *const Handle, index: u32) -> u32 {
    session!(handle, 0).session.gems_found(index as usize)
}

/// How many Archipelago gems each level of this run carries, which is the same
/// number for every level. 0 for a run that was set up without any.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_gems_per_level(handle: *const Handle) -> u32 {
    session!(handle, 0).session.gems_per_level()
}

/// How many of the moves upgrades for the level at `index` this run holds.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_moves(handle: *const Handle, index: u32) -> u32 {
    session!(handle, 0).session.moves_found(index as usize)
}

/// How many moves upgrades that level has to find. One today; asked for rather
/// than assumed, because the upgrade is going to become progressive.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_moves_total(handle: *const Handle, index: u32) -> u32 {
    session!(handle, 0).session.moves_total(index as usize)
}

/// How long a beaten level holds still before spending its leftover moves, so
/// the goals can be seen reaching their totals. Milliseconds; 0 for no hold,
/// which is what a board with no page in front of it does.
///
/// The page's number because the page owns the animation being waited for.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_set_goal_hold(handle: *mut Handle, ms: f32) {
    let handle = session_mut!(handle, ());
    handle.session.set_goal_hold(ms);
}

/// The best score this run has beaten the level at `index` with, or 0 if it
/// never has.
///
/// An `f64` for the same reason the score is: a run's total outgrows a `u32`
/// and the page has no integer wider than this one.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_level_best_score(handle: *const Handle, index: u32) -> f64 {
    session!(handle, 0.0).session.best_score(index as usize) as f64
}

/// Hands a best score back to a run being rebuilt from a save, the way
/// [`tg_restore`] hands back a location. Never announces anything, and never
/// lowers a score the run has already beaten the level with.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_restore_best_score(handle: *mut Handle, index: u32, score: f64) {
    let handle = session_mut!(handle, ());
    // A page is free to hand over nonsense; anything that is not a score is
    // no score at all.
    let score = if score.is_finite() && score > 0.0 { score as u64 } else { 0 };
    handle.session.restore_best_score(index as usize, score);
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

// ---- the multiworld -------------------------------------------------------

/// Hands what the locations hold over to a multiworld, or takes it back.
///
/// Set once a run is known to be a multiworld's. From then on checking a
/// location records the check and pays out nothing, because what was in it is
/// the server's to send; it comes back through [`tg_receive`]. Everything the
/// front end reads off the checked locations, the level marks and the gem
/// counts among them, carries on working unchanged.
///
/// Anything but 0 is on, so a caller passing a JavaScript boolean through
/// gets what it meant either way.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_set_remote(handle: *mut Handle, on: u32) {
    let handle = session_mut!(handle, ());
    handle.session.set_remote(on != 0);
}

/// Hands the run an item from the multiworld, by the engine's own item id.
///
/// 1 when the run is better off for it and 0 otherwise, which covers a number
/// no item has, an item for a level past the end of this ladder, and an unlock
/// that had already arrived. A server resending on a reconnect is the ordinary
/// way to see the last of those, so 0 is an answer rather than a complaint.
///
/// The id is the engine's own. Archipelago's is this plus [`tg_ap_id_base`],
/// and taking the offset off is the caller's job: the caller is the only part
/// of this that ever sees a number of the other kind.
///
/// Reports only what this call did, so handing over a list of fifty items
/// announces fifty items rather than one and a half thousand.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_receive(handle: *mut Handle, id: u32) -> u32 {
    let handle = session_mut!(handle, 0);
    let took = handle.session.receive_id(id);
    pack_run_events(handle);
    took as u32
}

/// Empties what the run is holding, leaving what it has checked alone.
///
/// For the sequence a reconnect takes: this, then every item the server listed.
/// Archipelago resends the list from the beginning each time it says hello, and
/// everything but the unlocks stacks, so adding it to what the run already held
/// would double it. Doing it this way round means neither side has to keep a
/// count of how many items it has already applied.
///
/// Quiet, like [`tg_restore`]: the list that follows is what says what the run
/// holds.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_forget_items(handle: *mut Handle) {
    let handle = session_mut!(handle, ());
    handle.session.forget_items();
    pack_run_events(handle);
}

/// Which generation of the world's data this build writes and plays by default.
///
/// Takes no handle: a fact about the game rather than about a run of it.
#[no_mangle]
pub extern "C" fn tg_generator() -> u32 {
    GENERATOR
}

/// Whether this build can play a seed that generator made.
///
/// Asked of the engine rather than answered by comparing numbers on the other
/// side, because which versions are playable is a list rather than a floor:
/// supporting one is a claim about having the code to read it, and that is not
/// automatically true of everything older.
#[no_mangle]
pub extern "C" fn tg_plays_generator(version: u32) -> u32 {
    plays_generator(version) as u32
}

/// What Archipelago's own item and location numbers are offset by.
///
/// Read rather than written down on the other side, so the two cannot drift.
/// Takes no handle: it is a fact about the game, not about a run of it.
#[no_mangle]
pub extern "C" fn tg_ap_id_base() -> u32 {
    AP_ID_BASE
}

/// Whether the run has finished the game, by whatever its goal setting asks.
///
/// The same rule the apworld gates its completion on, so what this says and
/// what a multiworld believes cannot disagree. A client tells the server on
/// the strength of it.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_goal_met(handle: *const Handle) -> u32 {
    session!(handle, 0).session.goal_met() as u32
}

/// A legal move packed as `r1 << 24 | c1 << 16 | r2 << 8 | c2`, or `u32::MAX`
/// when the board has none.
///
/// One of all the board allows rather than the first one found, so asking
/// twice may well name two different moves. That is why this takes the handle
/// by `*mut`: choosing draws from the run, which no amount of reading would.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_hint(handle: *mut Handle) -> u32 {
    let handle = session_mut!(handle, u32::MAX);
    match handle.session.game_mut().hint() {
        Some((a, b)) => {
            ((a.r as u32) << 24) | ((a.c as u32) << 16) | ((b.r as u32) << 8) | b.c as u32
        }
        None => u32::MAX,
    }
}

// ---- what the run is carrying --------------------------------------------

/// How many of one thing the run holds, by [`Consumable::code`]. 0 for a kind
/// the engine does not have, which is what a front end drawn against a newer
/// engine would ask about.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_consumables(handle: *const Handle, kind: u32) -> u32 {
    let handle = session!(handle, 0);
    Consumable::from_code(kind).map_or(0, |kind| handle.session.consumables(kind))
}

/// Spends one on a cell, returning 1 when it happened.
///
/// A row or column outside the board means no target at all, which is what
/// the cluster wants and what the other three refuse. The refusal matters: an
/// aimed one pointed somewhere it can do nothing, or any of them while the
/// board is busy, costs nothing rather than being thrown away.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_use_consumable(handle: *mut Handle, kind: u32, r: i32, c: i32) -> u32 {
    let handle = session_mut!(handle, 0);
    let Some(kind) = Consumable::from_code(kind) else { return 0 };
    let board = &handle.session.game().board;
    let on_the_board = r >= 0 && r < board.rows && c >= 0 && c < board.cols;
    let target = on_the_board.then(|| Pos::new(r, c));
    let spent = handle.session.use_consumable(kind, target);
    pack_events(handle);
    spent as u32
}

/// Hands a count back to a run being rebuilt from a save, the way
/// [`tg_restore`] hands back a location. Quiet, and it *sets* the count rather
/// than raising it: this is the one holding that goes down, so a run that has
/// spent two of five is restored to three.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_restore_consumables(handle: *mut Handle, kind: u32, held: u32) {
    let handle = session_mut!(handle, ());
    if let Some(kind) = Consumable::from_code(kind) {
        handle.session.restore_consumables(kind, held);
    }
}

/// How many rockets spent out of the inventory [`tg_flights_ptr`] has to
/// offer. 0 at every other moment, this being the only thing that flies from
/// nowhere on the board.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_flights_len(handle: *const Handle) -> u32 {
    (session!(handle, 0).session.game().flights().len() / 3) as u32
}

/// Three floats each: a column, a row, and 1 while it is still in the air.
/// See [`crate::game::Game::flights`].
///
/// # Safety
/// `handle` must come from [`tg_create`]. The pointer is valid until the next
/// call that mutates the session, so re-read it each frame.
#[no_mangle]
pub unsafe extern "C" fn tg_flights_ptr(handle: *const Handle) -> *const f32 {
    session!(handle, std::ptr::null()).session.game().flights().as_ptr()
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
    session!(handle, 0).session.game().objective_have(index as usize)
}

// ---- the debug menu ------------------------------------------------------
//
// Reached by tapping an objective chip twenty times over. Everything here
// grants what the run would have found rather than writing a result over the
// top of it: a level is opened by handing the run the unlock that opens it, so
// a forced run holds what an ordinary one would and every rule downstream
// still reads true. Filling the bonus inventory is the exception only because
// `tg_restore_consumables` already does exactly that.

/// Debug: open every level on the ladder.
///
/// Both ways a ladder can open, because the menu has no business knowing which
/// is in play: the counter covers a ladder that opens by clearing, and the
/// unlocks cover one that opens by item, where the counter is ignored and how
/// far a run may play is read off what it holds.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_unlock_all_levels(handle: *mut Handle) {
    let handle = session_mut!(handle, ());
    let levels = handle.session.level_count();
    handle.session.set_unlocked(levels);
    for _ in 1..levels {
        handle.session.receive(Item::LevelUnlock);
    }
}

/// Debug: give the run every special.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_unlock_all_specials(handle: *mut Handle) {
    let handle = session_mut!(handle, ());
    for special in UNLOCKS {
        handle.session.receive(Item::Unlock(special));
    }
}

/// Debug: declare the current level won, wherever the board is.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_force_clear(handle: *mut Handle) {
    let handle = session_mut!(handle, ());
    handle.session.game_mut().force_clear();
}

/// Debug: give the current level a different number of moves.
///
/// # Safety
/// `handle` must come from [`tg_create`].
#[no_mangle]
pub unsafe extern "C" fn tg_set_moves(handle: *mut Handle, moves: u32) {
    let handle = session_mut!(handle, ());
    handle.session.game_mut().set_moves(moves);
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
    // The board's own, then the run's. One stream: the page should not have to
    // ask two places what just happened.
    let mut events: Vec<Event> = handle.session.game().events().to_vec();
    events.extend_from_slice(handle.session.events());
    pack(handle, &events);
}

/// The run's own events and none of the board's, for a call that could not
/// have moved the board.
///
/// [`pack_events`] rebuilds the whole buffer from both sides every time, which
/// is right for a call that went through the board: the board clears its own
/// events as each call starts, so what is there is always what that call did.
/// A call that never reaches the board would find the board's last frame still
/// sitting there and hand it over a second time, which on this path means
/// replaying a whole cascade's worth of pops and debris for an item that
/// arrived while the board was settling.
fn pack_run_events(handle: &mut Handle) {
    let events: Vec<Event> = handle.session.events().to_vec();
    pack(handle, &events);
}

/// Writes events into the buffer the front end reads, replacing what was in it.
fn pack(handle: &mut Handle, events: &[Event]) {
    handle.events.clear();
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
    use crate::options::{setting_index, PROGRESSIVE_LEVELS, SETTINGS};
    use crate::progression::{Class, Location};

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

            // Played out rather than given a fixed number of frames, and
            // finished counts as at rest: which move `tg_hint` names is a draw
            // now, and the opening level is a three move puzzle, so one of
            // them can leave a board with nothing left to do. What this test
            // is about is that a move made through the ABI reaches the engine
            // and comes back, not where that particular board ends up.
            let at_rest = |phase: u32| phase == 0 || phase == 6;
            let mut frames = 0;
            while !at_rest(tg_phase(handle)) && frames < 4_000 {
                tg_update(handle, 16.0);
                frames += 1;
            }
            assert!(tg_score(handle) > 0.0, "the hinted move should have scored");
            assert!(at_rest(tg_phase(handle)), "the board never came back to rest");

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
            // A ladder that opens by clearing, set across the boundary the
            // way the page sets it when it reads a save. The count a save
            // hands back means nothing to a run opening it by item, which is
            // `a_ladder_that_opens_by_item_does_not_open_by_clearing`.
            let at = setting_index(PROGRESSIVE_LEVELS).expect("the setting exists");
            assert_eq!(tg_set_option(handle, at as u32, 0), 1, "the setting refused to go off");
            assert_eq!(tg_load_level(handle, 3), 0);
            tg_set_unlocked(handle, 5);
            assert_eq!(tg_unlocked(handle), 5);
            assert_eq!(tg_load_level(handle, 3), 1);
            assert_eq!(tg_level_index(handle), 3);
            tg_destroy(handle);
        }
    }

    /// The two status marks on a level's row in the picker, across the
    /// boundary: a fraction each, and both of them start empty.
    #[test]
    fn the_level_picker_can_read_what_is_still_out_there() {
        unsafe {
            let handle = tg_create(5, 0);
            let wanted = tg_gems_per_level(handle);
            assert!(wanted > 0, "this run hides no gems, so the mark would say nothing");
            assert_eq!(tg_level_gems(handle, 2), 0);
            assert_eq!(tg_level_moves(handle, 2), 0);
            assert!(tg_level_moves_total(handle, 2) > 0);

            tg_restore(handle, Location::ApGem { level: 2, index: 0 }.id());
            assert_eq!(tg_level_gems(handle, 2), 1);
            assert_eq!(tg_level_gems(handle, 1), 0, "it counted against every level");
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

    /// The feed colors an item's name by what the world makes of it, and it
    /// takes both off the same index. If those two lists ever drifted apart,
    /// every line would be colored by some other item's worth and nothing
    /// would look broken.
    #[test]
    fn a_name_and_what_it_is_worth_come_off_the_same_index() {
        unsafe {
            let handle = tg_create(5, 0);
            let bytes = std::slice::from_raw_parts(
                tg_item_names_ptr(handle),
                tg_item_names_len(handle) as usize,
            );
            let names: Vec<&str> = std::str::from_utf8(bytes).unwrap().split('\n').collect();
            let at = |wanted: &str| {
                names.iter().position(|name| *name == wanted).unwrap_or_else(|| {
                    panic!("the item list has no {wanted}, so this proves nothing")
                }) as u32
            };

            assert_eq!(tg_item_class(handle, at("Rocket")), Class::Progression.code());
            assert_eq!(tg_item_class(handle, at("Level 3 Moves Upgrade")), Class::Progression.code());
            assert_eq!(tg_item_class(handle, at("Filler")), Class::Filler.code());
            assert_eq!(
                tg_item_class(handle, at("Inventory Item: Rocket Cluster")),
                Class::Useful.code(),
            );
            // A page is free to ask about an item this engine does not have,
            // and the answer is the one that claims the least.
            assert_eq!(tg_item_class(handle, 9_999), Class::Filler.code());
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
                    // A toggle is a choice of two as far as the screen is
                    // concerned, and comes through carrying both of them. The
                    // word is its own because the yaml side does treat it
                    // differently: see `Kind::Toggle`.
                    "toggle" => assert_eq!(fields.len(), 7, "a toggle wants an off and an on"),
                    // The screen leaves these out, which is the point of
                    // them. They come across all the same, because what the
                    // page sets a setting by is its place in this list: one
                    // missing line would shift every setting after it.
                    "weight" => assert_eq!(fields.len(), 6, "a weight wants the group it is in"),
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
    fn a_consumable_crosses_the_boundary_and_is_only_spent_when_it_lands() {
        unsafe {
            let handle = tg_create(4321, 0);
            let rocket = Consumable::Rocket.code();
            assert_eq!(tg_consumables(handle, rocket), 0, "a fresh run is carrying one");

            tg_restore_consumables(handle, rocket, 2);
            assert_eq!(tg_consumables(handle, rocket), 2);

            // Off the board is no target at all, which an aimed one refuses.
            // The refusal has to leave the count alone: the whole point of
            // asking the board first is that a tap that can do nothing costs
            // nothing.
            assert_eq!(tg_use_consumable(handle, rocket, -1, -1), 0);
            assert_eq!(tg_use_consumable(handle, rocket, 0, tg_cols(handle) as i32), 0);
            assert_eq!(tg_consumables(handle, rocket), 2, "a refused tap was charged for");

            // Somewhere the opening board actually has a gem, since a level
            // may be any shape and a walled cell is nothing to shoot at.
            let cols = tg_cols(handle) as usize;
            let cells = std::slice::from_raw_parts(
                tg_cells_ptr(handle),
                (tg_rows(handle) * tg_cols(handle)) as usize * 4,
            );
            let at = cells.chunks(4).position(|c| c[0] != 255).expect("a gem on the board");
            assert_eq!(tg_use_consumable(handle, rocket, (at / cols) as i32, (at % cols) as i32), 1);
            assert_eq!(tg_consumables(handle, rocket), 1);

            // And it is in the air: no cell holds one spent out of the
            // inventory, so this list is the only place it shows up.
            assert_eq!(tg_phase(handle), 3, "spending a rocket did not start a launch");
            tg_update(handle, 8.0);
            assert_eq!(tg_flights_len(handle), 1);
            let flight = std::slice::from_raw_parts(tg_flights_ptr(handle), 3);
            assert!(flight[1] > (tg_rows(handle) - 1) as f32, "it did not set off below the board");
            assert_eq!(flight[2], 1.0, "it landed before it had flown anywhere");

            // A kind the engine does not have is answered rather than
            // mistaken for the first one, which is what a front end drawn
            // against a newer engine would ask about.
            assert_eq!(tg_consumables(handle, 99), 0);
            assert_eq!(tg_use_consumable(handle, 99, 0, 0), 0);
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
            tg_receive(null, 0);
            tg_set_remote(null, 1);
            tg_forget_items(null);
            assert_eq!(tg_goal_met(null), 0);
            tg_destroy(null);
        }
    }

    /// How many events the last call into the ABI reported.
    fn packed(handle: *const Handle) -> Vec<u8> {
        unsafe {
            std::slice::from_raw_parts(
                tg_events_ptr(handle),
                tg_events_len(handle) as usize * EVENT_SIZE,
            )
            .to_vec()
        }
    }

    #[test]
    fn a_received_item_reports_itself_and_nothing_the_board_did_earlier() {
        // A call that never reaches the board has to pack only its own news.
        // The board clears its events as each of its own calls starts, so what
        // was left there from the last frame would otherwise be handed over a
        // second time: an item arriving while gems were still falling would
        // replay the whole cascade's pops and debris to the page.
        unsafe {
            let handle = tg_create(7, 0);
            tg_set_remote(handle, 1);

            // Get the board busy and leave its events sitting in the engine.
            let hint = tg_hint(handle);
            let (r1, c1, r2, c2) = (
                (hint >> 24) as i32,
                ((hint >> 16) & 0xff) as i32,
                ((hint >> 8) & 0xff) as i32,
                (hint & 0xff) as i32,
            );
            assert_eq!(tg_swap(handle, r1, c1, r2, c2), 1);
            tg_update(handle, 16.0);
            assert!(!packed(handle).is_empty(), "the board raised nothing to be confused by");

            // One item, one event, and it is the item.
            let rainbow = crate::progression::Item::Unlock(Special::Rainbow);
            assert_eq!(tg_receive(handle, rainbow.id()), 1);
            let events = packed(handle);
            assert_eq!(events.len(), EVENT_SIZE, "a receive packed more than its own item");
            assert_eq!(events[0], crate::game::EV_ITEM);

            // And a second one does not bring the first along with it.
            let rocket = crate::progression::Item::Unlock(Special::Rocket);
            assert_eq!(tg_receive(handle, rocket.id()), 1);
            assert_eq!(packed(handle).len(), EVENT_SIZE, "the second receive replayed the first");

            // A refusal is quiet, and a resent unlock is a refusal.
            assert_eq!(tg_receive(handle, rocket.id()), 0);
            assert!(packed(handle).is_empty(), "a refused item announced something");

            tg_destroy(handle);
        }
    }

    #[test]
    fn a_multiworld_run_is_told_what_it_holds_rather_than_finding_it() {
        unsafe {
            let handle = tg_create(7, 0);
            tg_set_remote(handle, 1);
            assert_eq!(tg_unlocked(handle), 1);
            assert_eq!(tg_goal_met(handle), 0);

            // The ladder opens one level per item, and nothing else opens it.
            let unlock = crate::progression::Item::LevelUnlock;
            assert_eq!(tg_receive(handle, unlock.id()), 1);
            assert_eq!(tg_unlocked(handle), 2);
            assert_eq!(tg_receive(handle, unlock.id()), 1);
            assert_eq!(tg_unlocked(handle), 3);

            // And a reconnect: forget, be told again, land in the same place.
            tg_forget_items(handle);
            assert_eq!(tg_unlocked(handle), 1, "the forgetting left the ladder open");
            assert!(packed(handle).is_empty(), "the forgetting announced itself");
            tg_receive(handle, unlock.id());
            tg_receive(handle, unlock.id());
            assert_eq!(tg_unlocked(handle), 3, "being resent the list did not restore the ladder");

            // The offset the client works in. Read rather than written down on
            // the other side, so the two cannot drift.
            assert_eq!(tg_ap_id_base(), crate::progression::AP_ID_BASE);

            // And which seeds this build will take. Both of these cross the
            // ABI as numbers, so a client can refuse a seed at the door
            // without knowing anything about what a generation means.
            assert_eq!(tg_generator(), GENERATOR);
            assert_eq!(tg_plays_generator(GENERATOR), 1);
            assert_eq!(tg_plays_generator(GENERATOR + 1), 0);
            assert_eq!(tg_plays_generator(u32::MAX), 0);

            tg_destroy(handle);
        }
    }
}
