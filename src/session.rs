//! A run through the level ladder.
//!
//! [`Session`] is the seam where progression lives: today it unlocks the next
//! level when you clear one, and later the Archipelago layer will answer the
//! same question (which levels may be played) from received items instead.

use crate::board::Pos;
use crate::game::{Event, Game, Status, EV_AP_CLEAR, EV_ITEM};
use crate::level::{levels, LevelSpec};
use crate::options::{Kind, Options, SETTINGS};
use crate::progression::{
    ap_gems_per_level, fill_seed, item_index, item_name, item_pool, items, location_index,
    location_name, locations, solo_placement, Class, Consumable, Inventory, Item, Location, Tier,
    LONGEST_CHAIN, LONGEST_MATCH, NO_LOCATION, SHORTEST_CHAIN, SHORTEST_MATCH,
};
use crate::rng::Rng;

pub struct Session {
    levels: Vec<LevelSpec>,
    index: usize,
    /// How many levels the player may pick from, counting from the first.
    unlocked: usize,
    /// Deals each level's board seed, in the order the levels are started.
    ///
    /// Starting a level draws the next one rather than deriving it from the
    /// level's place in the ladder, so coming back to a level deals a fresh
    /// board. A seed that happens to lay out badly is then one restart away
    /// from a better one instead of something to grind against, which matters
    /// once a level can be replayed for a score.
    ///
    /// The run as a whole stays reproducible: the same run seed walked through
    /// the same levels in the same order deals the same boards, which is what
    /// the tests and the screenshot tooling rely on.
    deal: Rng,
    /// What the run has been given. A solo run finds it by clearing levels;
    /// under Archipelago the same inventory is filled from the multiworld.
    inventory: Inventory,
    /// Locations this run has already checked, by id.
    ///
    /// The record that stops a location paying twice, which matters now that
    /// move items stack: without it, reaching a three long chain again would
    /// keep handing moves over. Written into the save so a returning run keeps
    /// what it found, and keeps not being able to find it again.
    checked: Vec<u32>,
    /// What each location holds, dealt once when the run opens from the run's
    /// own seed. Working it out walks the whole ladder by reachability, which
    /// is not something to do on the frame a level is cleared.
    placement: Vec<Option<Item>>,
    /// The best score each level has been beaten with, this run, by level.
    ///
    /// Kept beside the marks rather than worked out on the page, because when
    /// a score counts is the same question the marks answer and it should not
    /// have two answers: a level is beaten from the moment it is cleared, the
    /// score climbs through the flourish after that, and an attempt that ran
    /// out of moves scored nothing that any of this is about.
    ///
    /// Zero means the level has never been beaten. Not part of the
    /// progression, so no location holds it and no rule reads it.
    best_scores: Vec<u64>,
    /// How long a beaten level holds still while the page finishes showing
    /// its goals met; see [`Phase::Tallying`]. The page's number, handed over
    /// once and applied to every level this run deals.
    goal_hold_ms: f32,
    /// What the run was dealt from, kept so changing a setting can deal the
    /// same run again rather than a different one.
    seed: u64,
    /// What sort of run this is. Fixed once it has started: see
    /// [`Session::set_option`].
    options: Options,
    /// Things that happened to the run rather than to the board, raised
    /// alongside the board's own so the page has one stream to watch.
    events: Vec<Event>,
    game: Game,
    name_buf: Vec<u8>,
    names_blob: Vec<u8>,
    item_names_blob: Vec<u8>,
    location_names_blob: Vec<u8>,
    option_table_blob: Vec<u8>,
}

/// The settings table as one line per setting. See [`Session::option_table`].
fn option_table_text() -> Vec<u8> {
    let lines = SETTINGS.iter().map(|setting| {
        let mut fields = vec![
            setting.key.to_string(),
            setting.label.to_string(),
            setting.about.to_string(),
        ];
        match setting.kind {
            // The step is left out on purpose: the screen asks the engine to
            // move a setting one step rather than working the new value out
            // itself, so it is the engine's business how far a step is. See
            // `tg_step_option`.
            Kind::Range { low, high, step: _ } => {
                fields.push("range".to_string());
                fields.push(setting.default.to_string());
                fields.push(low.to_string());
                fields.push(high.to_string());
            }
            Kind::Choice(choices) => {
                fields.push("choice".to_string());
                fields.push(setting.default.to_string());
                for choice in choices {
                    fields.push(format!("{}={}", choice.value, choice.label));
                }
            }
        }
        fields.join("\t")
    });
    blob(lines)
}

/// Joins names into the newline separated blob the ABI hands over.
fn blob(names: impl Iterator<Item = String>) -> Vec<u8> {
    names.collect::<Vec<_>>().join("\n").into_bytes()
}

impl Session {
    /// Opens a run set up however a fresh one is set up.
    pub fn new(seed: u64) -> Self {
        Session::set_up(seed, Options::default())
    }

    /// Opens a run. The seed is the whole run: it deals every board, and it
    /// deals the progression, so two players on the same seed play the same
    /// game and nobody else plays theirs. The options are what sort of run it
    /// is, and they are fixed for its whole length.
    pub fn set_up(seed: u64, options: Options) -> Self {
        let levels = levels();
        let first = levels.first().expect("the ladder is never empty");
        let mut deal = Rng::new(seed);
        // A run starts holding nothing, so the opener plays as plain matching
        // however much its own rules would otherwise allow.
        let inventory = Inventory::empty();
        let mut session = Session {
            // A fresh run has checked nothing, so the opening level carries
            // its whole allowance.
            game: open_level(
                0,
                first,
                &inventory,
                deal.next_u64(),
                ApGems {
                    odds: options.ap_gem_odds,
                    wanted: ap_gems_per_level(levels.len(), &options),
                },
            ),
            levels: levels.clone(),
            index: 0,
            unlocked: 1,
            deal,
            inventory,
            checked: Vec::new(),
            placement: solo_placement(levels.len(), fill_seed(seed), &options),
            best_scores: vec![0; levels.len()],
            goal_hold_ms: 0.0,
            seed,
            options,
            events: Vec::new(),
            name_buf: Vec::new(),
            names_blob: Vec::new(),
            item_names_blob: Vec::new(),
            location_names_blob: Vec::new(),
            option_table_blob: option_table_text(),
        };
        let count = session.levels.len();
        session.names_blob =
            blob(session.levels.iter().map(|level| level.name.to_string()));
        session.item_names_blob = blob(items(count).into_iter().map(item_name));
        session.location_names_blob = blob(locations(count).into_iter().map(location_name));
        session.sync_name();
        session
    }

    pub fn game(&self) -> &Game {
        &self.game
    }

    /// What sort of run this is.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Changes one setting, which starts the run over.
    ///
    /// Not an edit to a game in progress: the settings decide how many items
    /// there are and what the rules ask for, so a run half played under one
    /// set and half under another is not a run anybody could describe. What
    /// this does is deal the same seed again under the new setting, which is
    /// the only honest reading of changing your mind before you start.
    ///
    /// Returns whether it took. A value the setting does not allow leaves
    /// everything alone.
    pub fn set_option(&mut self, at: usize, value: u32) -> bool {
        let mut wanted = self.options;
        if !wanted.set(at, value) {
            return false;
        }
        if wanted == self.options {
            return true;
        }
        *self = Session::set_up(self.seed, wanted);
        true
    }

    /// How many settings there are, for a front end walking the table.
    pub fn option_count(&self) -> usize {
        SETTINGS.len()
    }

    /// The settings table as text, one line per setting, for a front end to
    /// build a screen out of without knowing what is in it.
    ///
    /// Tab separated: key, label, the sentence about it, `range` or `choice`,
    /// the default, then the two bounds for a range or `value=label` for each
    /// of a choice's values. The engine writes it for the same reason it
    /// writes the item names: a screen that knew the settings would need
    /// editing every time one was added, and would drift when somebody forgot.
    pub fn option_table(&self) -> &[u8] {
        &self.option_table_blob
    }

    /// One step along a setting, which is the engine's business rather than
    /// the screen's: a range stops at its ends and a choice goes round.
    pub fn step_option(&self, at: usize, by: i32) -> u32 {
        let Some(setting) = SETTINGS.get(at) else { return 0 };
        setting.step(self.options.get(at).unwrap_or(setting.default), by)
    }

    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    pub fn index(&self) -> usize {
        self.index
    }

    /// How many levels the player may pick from, counting from the first.
    ///
    /// Two different answers, because there are two ways to open the ladder. A
    /// run opening it by clearing keeps the count as it goes and this reports
    /// it. A run opening it by item does not keep a count at all: what it
    /// holds is the answer, and one more level is open for each Progressive
    /// Level Unlock it has been handed. Worked out rather than recorded so the
    /// two can never disagree, the way [`Session::best_tier`] is read off the
    /// checked locations rather than kept alongside them.
    pub fn unlocked(&self) -> usize {
        if self.options.progressive_levels == 0 {
            return self.unlocked;
        }
        let held = self.inventory.count(Item::LevelUnlock) as usize;
        (1 + held).min(self.levels.len())
    }

    pub fn level_name(&self) -> &[u8] {
        &self.name_buf
    }

    /// Every level's name, newline separated, so a front end can build a level
    /// picker without a call per entry.
    pub fn level_names(&self) -> &[u8] {
        &self.names_blob
    }

    /// Every item's name, in the order [`items`] gives them.
    ///
    /// The engine owns these because they are the same strings a tracker and a
    /// spoiler log will show. A front end that built its own would drift from
    /// them the first time either side was edited, and the symptom would be a
    /// player's feed disagreeing with their tracker.
    pub fn item_names(&self) -> &[u8] {
        &self.item_names_blob
    }

    /// What the item at `index` in that same list counts as, which is what
    /// colors its name in the feed.
    ///
    /// Filler for an index off the end, because that is the answer that claims
    /// the least about an item this engine does not have.
    pub fn item_class(&self, index: usize) -> Class {
        items(self.levels.len()).get(index).map_or(Class::Filler, |item| item.class())
    }

    /// Every location's name, in the order [`locations`] gives them.
    pub fn location_names(&self) -> &[u8] {
        &self.location_names_blob
    }

    /// Switches to a level the player has unlocked. Returns false for one they
    /// have not, or for an index off the end of the ladder.
    pub fn load(&mut self, index: usize) -> bool {
        if index >= self.levels.len() || index >= self.unlocked() {
            return false;
        }
        self.index = index;
        self.deal_level();
        true
    }

    /// Moves on after a win. Returns false when the ladder is finished, and
    /// also when the next level is not open: a run unlocking by item can beat
    /// a level and have nowhere to go until one turns up.
    ///
    /// Nothing crosses the ABI for this any more. The panel at the end of a
    /// level stopped offering to go on, because with the ladder opening by
    /// item there is not always an onward to offer and the level select is
    /// where that question belongs. Kept because the ladder is a ladder and
    /// "the next one" is a thing to be able to ask for; `tg_load_level` is
    /// what the page uses.
    pub fn next_level(&mut self) -> bool {
        let next = self.index + 1;
        if next >= self.levels.len() {
            return false;
        }
        // Only the count a clear keeps. A run opening the ladder by item
        // ignores this, and `load` will refuse the level it has not been
        // handed.
        self.unlocked = self.unlocked.max(next + 1);
        self.load(next)
    }

    /// Restores how far a returning player had got. Clamped to the ladder, and
    /// never used to lock something already unlocked this run.
    ///
    /// A run opening the ladder by item writes this down and then ignores it
    /// on the way back in: the unlocks come back with the checked locations,
    /// and what the run holds is the answer. A save that claimed more levels
    /// than its items account for cannot open them.
    pub fn set_unlocked(&mut self, count: usize) {
        self.unlocked = self.unlocked.max(count.clamp(1, self.levels.len()));
    }

    /// Plays the current level again on a freshly dealt board.
    ///
    /// Not [`Game::restart`], which replays the same deal: a player who asks
    /// for another go at a level they could not clear wants another board, not
    /// the one that just beat them.
    pub fn retry(&mut self) {
        self.deal_level();
    }

    /// Opens the current level on the next board the run has to give, under
    /// whatever the run holds by now.
    fn deal_level(&mut self) {
        let seed = self.deal.next_u64();
        let gems = self.ap_gems_for(self.index);
        self.game =
            open_level(self.index, &self.levels[self.index], &self.inventory, seed, gems);
        self.game.goal_hold_ms = self.goal_hold_ms;
        self.sync_name();
    }

    /// How many checks are still waiting in one level's gems, and how often
    /// one should fall in.
    fn ap_gems_for(&self, index: usize) -> ApGems {
        let per_level = ap_gems_per_level(self.levels.len(), &self.options);
        let taken = (0..per_level)
            .filter(|i| self.checked.contains(&Location::ApGem { level: index, index: *i }.id()))
            .count() as u32;
        ApGems { odds: self.options.ap_gem_odds, wanted: per_level - taken }
    }

    /// The level being played, as the ladder defines it.
    ///
    /// The ladder's own answer, not the narrowed copy the board is running:
    /// what is wanted from here is the level's design (its score tiers, say),
    /// which is the same whatever the run happens to hold.
    pub fn level(&self) -> &LevelSpec {
        &self.levels[self.index]
    }

    /// What the run may make. The front end shows it; the Archipelago layer
    /// will answer the same question from received items.
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// Takes an item in from outside the ladder, which is how the multiworld
    /// will deliver. Returns whether it was new.
    ///
    /// It reaches the board in front of the player rather than waiting for the
    /// next deal: an unlock that arrives mid level should be usable in that
    /// level, which is what receiving it means.
    pub fn receive(&mut self, item: Item) -> bool {
        self.receive_from(item, None)
    }

    /// The same, saying where it came from so the announcement can too.
    ///
    /// `None` is an item that came from no location on this board, which is
    /// what a multiworld handing one over looks like.
    fn receive_from(&mut self, item: Item, from: Option<Location>) -> bool {
        let is_new = self.inventory.receive(item);
        if is_new {
            self.refresh_specials();
            self.refresh_moves();
            let levels = self.levels.len();
            let at = from
                .and_then(|location| location_index(location, levels))
                .map_or(NO_LOCATION, |index| index as u16);
            let which = item_index(item, levels).unwrap_or(0) as u16;
            self.events.push(Event::about_item(EV_ITEM, at, which));
        }
        is_new
    }

    /// Everything that happened to the run during the last call, which the ABI
    /// packs alongside the board's own events.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Re-derives what the board in play may make, from the level's own answer
    /// and what the run holds now.
    ///
    /// Reading it back from the level rather than narrowing the live rules
    /// again is the whole of it. Those have already been cut down once, and
    /// intersecting them with a larger inventory can only ever take more away:
    /// an item would arrive and change nothing.
    fn refresh_specials(&mut self) {
        let mut spec = self.levels[self.index].clone();
        self.inventory.apply(self.index, &mut spec);
        self.game.spec.rules.specials = spec.rules.specials;
    }

    /// Re-derives the level's move budget, handing the player the difference.
    ///
    /// A moves item that arrives during a level is worth its moves in that
    /// level, so what it adds goes on the counter in front of the player as
    /// well as on the budget. Anything already spent stays spent.
    fn refresh_moves(&mut self) {
        let mut spec = self.levels[self.index].clone();
        self.inventory.apply(self.index, &mut spec);
        let extra = spec.moves.saturating_sub(self.game.spec.moves);
        self.game.spec.moves = spec.moves;
        self.game.moves_left += extra;
    }

    /// Takes whatever is at a location, once and once only.
    ///
    /// Has to run every frame a board sits won and every frame a chain stands
    /// at its longest, so the record of what has been checked is what keeps it
    /// honest rather than the caller being careful.
    fn check(&mut self, location: Location) {
        let id = location.id();
        if self.checked.contains(&id) {
            return;
        }
        self.checked.push(id);
        if let Some(item) = self.holds(location) {
            self.receive_from(item, Some(location));
        }
    }

    /// Which locations this run has checked, for writing down.
    pub fn checked(&self) -> &[u32] {
        &self.checked
    }

    /// What one location is holding in this run. Another run holding a
    /// different seed will have something else there.
    pub fn holds(&self, location: Location) -> Option<Item> {
        location_index(location, self.levels.len()).and_then(|at| self.placement[at])
    }

    /// How well the level at `index` has been beaten, at best.
    ///
    /// Read back off the checked locations, so what the level select shows and
    /// what the run has actually found cannot drift apart.
    /// Spends one of the things the run is carrying, reporting whether it
    /// happened.
    ///
    /// The board is asked first and the item is taken out only if the board
    /// took it, so a tap that could do nothing costs nothing: an aimed one
    /// pointed at a cell it cannot work on, or any of them while the board is
    /// busy, is refused with the inventory untouched.
    pub fn use_consumable(&mut self, kind: Consumable, target: Option<Pos>) -> bool {
        if self.inventory.consumables(kind) == 0 {
            return false;
        }
        if !self.game.use_consumable(kind, target) {
            return false;
        }
        self.inventory.spend(kind);
        true
    }

    /// How many of one thing the run is carrying.
    pub fn consumables(&self, kind: Consumable) -> u32 {
        self.inventory.consumables(kind)
    }

    /// Hands a count back to a run being rebuilt from a save. See
    /// [`Inventory::restore_consumables`] on why this sets rather than raises.
    pub fn restore_consumables(&mut self, kind: Consumable, held: u32) {
        self.inventory.restore_consumables(kind, held);
    }

    /// How long a beaten level holds still before its flourish starts, so the
    /// goals can be seen reaching their totals. See [`Phase::Tallying`].
    ///
    /// Takes effect on the level in play as well as every one dealt after it,
    /// because the page sets it once at startup and a level is already open by
    /// then.
    pub fn set_goal_hold(&mut self, ms: f32) {
        self.goal_hold_ms = if ms.is_finite() && ms > 0.0 { ms } else { 0.0 };
        self.game.goal_hold_ms = self.goal_hold_ms;
    }

    /// The best score this run has beaten the level at `index` with, or 0 if
    /// it never has. See [`Session::best_scores`].
    pub fn best_score(&self, index: usize) -> u64 {
        self.best_scores.get(index).copied().unwrap_or(0)
    }

    /// Hands a best score back to a run being rebuilt from a save.
    ///
    /// Only ever upward, the way the live one moves: a save cannot take away
    /// something this run has already beaten a level with, and restoring the
    /// same save twice cannot lower it.
    pub fn restore_best_score(&mut self, index: usize, score: u64) {
        if let Some(best) = self.best_scores.get_mut(index) {
            *best = (*best).max(score);
        }
    }

    /// How many of the Archipelago gems on the level at `index` this run has
    /// already taken.
    ///
    /// Read off the checked locations the way [`Session::best_tier`] is, so
    /// what a tracker shows and what the run has found cannot drift apart.
    pub fn gems_found(&self, index: usize) -> u32 {
        self.checked
            .iter()
            .filter_map(|id| Location::from_id(*id))
            .filter(|at| matches!(at, Location::ApGem { level, .. } if *level == index))
            .count() as u32
    }

    /// How many Archipelago gems every level of this run carries.
    ///
    /// One number for the whole ladder, because that is how a run is set up:
    /// see [`ap_gems_per_level`]. Zero is a real answer, for a run whose pool
    /// is small enough not to need any.
    pub fn gems_per_level(&self) -> u32 {
        ap_gems_per_level(self.levels.len(), &self.options)
    }

    /// How many of the level's moves upgrades this run is holding.
    pub fn moves_found(&self, index: usize) -> u32 {
        self.inventory.moves_found(index)
    }

    /// How many there are to find for that level.
    ///
    /// Counted out of the pool rather than answered with one, because the pool
    /// is where it is decided, and it is what changes when the upgrade becomes
    /// progressive.
    pub fn moves_total(&self, index: usize) -> u32 {
        item_pool(self.levels.len(), self.seed, &self.options)
            .iter()
            .filter(|item| matches!(item, Item::Moves { level } if *level == index))
            .count() as u32
    }

    pub fn best_tier(&self, index: usize) -> Tier {
        let reached = |at: Location| self.checked.contains(&at.id());
        if reached(Location::LevelGold(index)) {
            Tier::Gold
        } else if reached(Location::LevelSilver(index)) {
            Tier::Silver
        } else if reached(Location::LevelClear(index)) {
            Tier::Clear
        } else {
            Tier::None
        }
    }

    /// Hands a run back a location it had already checked, rebuilding what it
    /// was worth without announcing it again.
    ///
    /// The quiet half of [`Session::check`]: restoring a save should leave the
    /// run holding what it held, not replay every item it ever found through
    /// the feed. Unknown ids are ignored, which is what a save written by a
    /// later version looks like from here.
    pub fn restore(&mut self, id: u32) {
        let Some(location) = Location::from_id(id) else { return };
        // Whether this ladder actually has that location, asked of the one
        // function that knows. What stood here was a kind-by-kind bounds
        // check, a third copy of the same question, and it went stale the
        // moment a new kind of location was added: an id far past the end of
        // the table read back as a gem on level six hundred and was believed.
        if location_index(location, self.levels.len()).is_none() {
            return;
        }
        if self.checked.contains(&id) {
            return;
        }
        self.checked.push(id);
        if let Some(item) = self.holds(location) {
            self.inventory.receive(item);
        }
        self.refresh_specials();
        self.refresh_moves();
    }

    /// Checks whatever the board has earned this frame.
    ///
    /// From the moment the level is cleared, not from the moment it is over.
    /// The flourish in between is still play: the score climbs all the way
    /// through it, so a mark can be crossed in there, and an unlock that
    /// crossing it pays for is a special the rest of the flourish can mint.
    /// Waiting for the board to stop would hand it over after the one thing it
    /// could have changed. The score only ever climbs, so checking every frame
    /// reaches the same marks in the end and reaches them sooner.
    ///
    /// This is how the multiworld behaves too, which is the reason it has to
    /// be how this does: items arrive from other worlds whenever they arrive,
    /// and one that lands during the flourish should land on the board in
    /// front of the player rather than after it.
    fn check_reached(&mut self) {
        if self.game.cleared() {
            self.check(Location::LevelClear(self.index));
            let (silver, gold) = (self.levels[self.index].silver, self.levels[self.index].gold);
            let score = self.game.progress.score;
            // Taken every frame alongside the marks, and for the same reason:
            // the score is still climbing through the flourish, so the number
            // a beaten level ends on is not the one it was cleared with.
            let best = &mut self.best_scores[self.index];
            *best = (*best).max(score);
            if silver > 0 && score >= silver {
                self.check(Location::LevelSilver(self.index));
            }
            if gold > 0 && score >= gold {
                self.check(Location::LevelGold(self.index));
            }
        }
        // A chain that got to five got to two, three and four on the way.
        let reached = self.game.cascade().min(LONGEST_CHAIN);
        for length in SHORTEST_CHAIN..=reached {
            self.check(Location::Chain(length));
        }
        // A match does not work that way: each size is its own shape, so a
        // five pays the five and nothing else. The board's number is the
        // player's own last swap, which is what these are about; a cascade
        // lining one up afterwards is the board's doing, not theirs.
        let lined_up = self.game.swap_match();
        if (SHORTEST_MATCH..=LONGEST_MATCH).contains(&lined_up) {
            self.check(Location::Match(lined_up));
        }
        for _ in 0..self.game.events().iter().filter(|e| e.kind == EV_AP_CLEAR).count() {
            self.check_next_ap_gem();
        }
    }

    /// Checks the next Archipelago gem in this level's sequence.
    ///
    /// Which gem was collected on the board says nothing about which check it
    /// pays: they go in order, and the order belongs to the run rather than to
    /// the playthrough. Coming back to a level whose first two gems are
    /// already checked and clearing one there takes the third, not the first.
    ///
    /// Nothing happens once the level's run is exhausted. A run asking for one
    /// gem a level has one check there however many gems it goes on to clear,
    /// which is what keeps the pool and the locations agreeing.
    fn check_next_ap_gem(&mut self) {
        let gems = ap_gems_per_level(self.levels.len(), &self.options);
        for index in 0..gems {
            let at = Location::ApGem { level: self.index, index };
            if !self.checked.contains(&at.id()) {
                self.check(at);
                return;
            }
        }
    }

    pub fn update(&mut self, dt_ms: f32) {
        // Cleared here rather than drained by the reader, to match how the
        // board reports: everything raised during this call, and nothing else.
        self.events.clear();
        self.game.update(dt_ms);
        if self.game.status() == Status::Won {
            // What clearing a level opens. A run unlocking by item reads none
            // of this: see `unlocked`.
            self.unlocked = self.unlocked.max((self.index + 2).min(self.levels.len()));
        }
        self.check_reached();
    }

    pub fn game_mut(&mut self) -> &mut Game {
        &mut self.game
    }

    fn sync_name(&mut self) {
        self.name_buf.clear();
        self.name_buf.extend_from_slice(self.levels[self.index].name.as_bytes());
    }
}

/// Opens a level narrowed to what the run may actually do.
///
/// The one place a [`Game`] is built from a [`LevelSpec`], so there is nowhere
/// for a level to be started with more than the run has earned.
fn open_level(
    index: usize,
    spec: &LevelSpec,
    inventory: &Inventory,
    seed: u64,
    gems: ApGems,
) -> Game {
    let mut spec = spec.clone();
    inventory.apply(index, &mut spec);
    spec.rules.ap_gem_odds = gems.odds;
    let mut game = Game::new(spec, seed);
    game.ap_gems_wanted = gems.wanted;
    game
}

/// What a level should do about Archipelago gems: how often one falls in, and
/// how many checks are still waiting in them here.
///
/// Worked out from the run's options and from what it has already checked, so
/// coming back to a level whose gems are all taken deals a board with none,
/// and coming back partway through deals one with the rest still to find.
#[derive(Clone, Copy)]
struct ApGems {
    odds: u32,
    wanted: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Gem, Pos, Special};
    use crate::game::Phase;
    use crate::progression::CONSUMABLES;
    use crate::level::Objective;
    use crate::rules::SpecialSet;

    /// Wins the level in front of the session without playing it properly.
    ///
    /// The score is put out of reach and then the board is nudged until it
    /// ends, because what these tests are about is what a clear leads to, not
    /// whether the level is beatable.
    /// A run whose ladder opens by clearing, which is not what a fresh one
    /// does any more.
    ///
    /// Most of what follows is about something other than the ladder and wants
    /// one it can simply walk up: a test that stops at the second level
    /// because it has not been handed an item is a test about the wrong thing.
    /// The two ways the ladder opens are checked on their own, in
    /// `a_ladder_that_opens_by_item_does_not_open_by_clearing` and its pair.
    fn climbing(seed: u64) -> Session {
        Session::set_up(seed, Options { progressive_levels: 0, ..Options::default() })
    }

    /// What an item event says, unpacked: which item, and where from.
    type Announced = (u16, u16);

    fn announced(events: &[Event]) -> Vec<Announced> {
        events
            .iter()
            .filter(|e| e.kind == EV_ITEM)
            .map(|e| (e.value, e.color as u16 | ((e.special as u16) << 8)))
            .collect()
    }

    /// The same, built from the item and location themselves, so a test can
    /// say what it expects in those terms rather than in numbers.
    fn says(item: Item, from: Option<Location>) -> Announced {
        let levels = levels().len();
        (
            item_index(item, levels).expect("the item is in the table") as u16,
            from.map_or(NO_LOCATION, |at| {
                location_index(at, levels).expect("the location is in the table") as u16
            }),
        )
    }

    /// Wins the level in front of the session without playing it properly,
    /// gathering whatever it announced along the way.
    ///
    /// The score is put out of reach and then the board is nudged until it
    /// ends, because what these tests are about is what a clear leads to, not
    /// whether the level is beatable. Events are collected as they go: the
    /// session clears them every frame, like the board does.
    /// Wins the level with the score held at exactly `score` the whole way.
    ///
    /// Pinned rather than set once, because the board keeps scoring as it
    /// settles and the flourish keeps scoring after that: a test about a
    /// score threshold would drift off its own number otherwise.
    fn force_win_at(session: &mut Session, score: u64) {
        while session.game().phase() != Phase::Finished {
            meet_everything_but_the_score(session.game_mut());
            session.game_mut().progress.score = score;
            if let Some((a, b)) = first_move(session) {
                session.game_mut().try_swap(a, b);
            }
            session.update(16.0);
        }
        assert_eq!(session.game().status(), Status::Won);
        assert_eq!(session.game().progress.score, score, "the score did not stay put");
    }

    /// A run whose opening level's clear holds one of the unlocks.
    ///
    /// Which item a location holds is the run's own business now, so a test
    /// about what finding an unlock does has to go looking for a run that
    /// finds one. Five unlocks are dealt among the eighteen places a run can
    /// reach holding nothing, so most seeds put one here and a handful is
    /// plenty to look through.
    fn run_finding_an_unlock() -> (Session, Special) {
        // On a ladder that opens by clearing, like its pair below. What the
        // opening clear pays is the whole point of both, and with the ladder
        // opening by item it mostly pays a level unlock: the fill puts those
        // in first, because everything waits on them.
        for seed in 0..64 {
            let session = climbing(seed);
            if let Some(Item::Unlock(special)) = session.holds(Location::LevelClear(0)) {
                return (session, special);
            }
        }
        panic!("no run in the first 64 seeds finds an unlock by clearing the opening level");
    }

    /// The other half: a run whose opening clear pays only moves.
    ///
    /// Anything that pins a score needs one. A run that finds an unlock by
    /// clearing can mint with it straight away, and the flourish then scores
    /// on top of whatever the score was pinned to.
    fn run_finding_no_unlock() -> Session {
        for seed in 0..64 {
            let session = climbing(seed);
            if let Some(Item::Moves { .. }) = session.holds(Location::LevelClear(0)) {
                return session;
            }
        }
        panic!("no run in the first 64 seeds pays only moves for the opening level");
    }

    /// The first legal move on the board, which is what these tests steer by.
    ///
    /// Not `hint`: that chooses at random from everything the board allows,
    /// so a test playing by it would be measuring the draw. What is wanted
    /// here is a bot that plays the same way every time.
    fn first_move(session: &mut Session) -> Option<(Pos, Pos)> {
        crate::matching::find_move(&session.game().board, session.game().rules())
    }

    /// Marks every objective but the score as met, wherever the level's goals
    /// are not about the score at all.
    ///
    /// A level ends when its objectives are met, and those are not all one
    /// kind: a score can be put out of reach with one assignment, but a color
    /// count, a board of jelly or a wall of brick cannot. These tests are
    /// about what a clear leads to rather than about whether a level is
    /// beatable, so the level is simply told it has been beaten.
    ///
    /// Re-applied every frame, because the board recounts what is left of
    /// itself as it settles.
    fn meet_everything_but_the_score(game: &mut Game) {
        for objective in game.objectives().to_vec() {
            match objective {
                Objective::Score(_) => {}
                Objective::Color { color, count } => {
                    if let Some(slot) = game.progress.cleared.get_mut(color as usize) {
                        *slot = (*slot).max(count);
                    }
                }
                Objective::Jelly => game.progress.jelly_left = 0,
                Objective::Brick => game.progress.brick_left = 0,
                Objective::Seal { color } => {
                    if let Some(slot) = game.progress.seals_now.get_mut(color as usize) {
                        *slot = 0;
                    }
                }
            }
        }
    }

    fn force_win(session: &mut Session) -> Vec<Announced> {
        let mut seen = Vec::new();
        session.game_mut().progress.score = 1_000_000;
        while session.game().phase() != Phase::Finished {
            meet_everything_but_the_score(session.game_mut());
            if let Some((a, b)) = first_move(session) {
                session.game_mut().try_swap(a, b);
            }
            session.update(16.0);
            seen.extend(announced(session.events()));
        }
        assert_eq!(session.game().status(), Status::Won);
        seen
    }

    #[test]
    fn a_new_run_plays_the_opener_as_plain_matching() {
        // The level itself offers everything now; holding none of the unlocks
        // is what makes the opening board leave nothing behind.
        let session = Session::new(7);
        assert_eq!(session.level_name(), b"First Light");
        assert!(
            session.game().rules().specials.is_empty(),
            "a run that has been given nothing should make nothing",
        );
        assert!(session.events().is_empty());
    }

    #[test]
    fn clearing_a_level_hands_over_what_it_is_holding_and_says_where_from() {
        // Which item that is belongs to the placement, not to this test: the
        // fill decides, and asking for a particular one here would only pin
        // the fill's current shape rather than the behavior.
        let mut session = Session::new(7);
        let held = session.holds(Location::LevelClear(0)).expect("the opener holds something");
        let seen = force_win(&mut session);
        assert!(
            seen.contains(&says(held, Some(Location::LevelClear(0)))),
            "clearing the opener did not hand over {} and say where from",
            item_name(held),
        );
        assert!(session.inventory().has(held));
    }

    #[test]
    fn a_cleared_level_pays_its_location_only_the_first_time() {
        // `update` sees a won board on every frame until something else
        // happens, so collecting has to survive being asked repeatedly.
        let mut session = Session::new(7);
        let held = session.holds(Location::LevelClear(0)).expect("the opener holds something");
        let first = force_win(&mut session);
        assert!(first.contains(&says(held, Some(Location::LevelClear(0)))));
        session.retry();
        let again = force_win(&mut session);
        // Other locations may well pay on the way (the end-of-level flourish
        // can chain, and chains are locations too). What must not happen is
        // this level's own clear paying a second time.
        let clear = location_index(Location::LevelClear(0), levels().len()).unwrap() as u16;
        assert!(
            !again.iter().any(|(_, from)| *from == clear),
            "clearing the same level paid its location twice",
        );
        assert!(session.inventory().has(held), "and it kept what it found");
    }

    #[test]
    fn lining_up_gems_checks_that_size_and_only_that_size() {
        // Each size is its own shape, so a five pays the five and leaves the
        // four for somebody who lines up a four.
        let mut session = Session::new(7);
        let matches = |session: &Session| -> Vec<u32> {
            session
                .checked()
                .iter()
                .filter_map(|id| match Location::from_id(*id) {
                    Some(Location::Match(gems)) => Some(gems),
                    _ => None,
                })
                .collect()
        };
        assert!(matches(&session).is_empty(), "a run that has swapped nothing has matched");

        // Laid on rather than played for, because which boards a seed deals
        // is its own business and this is about what a given match pays.
        let board = &mut session.game_mut().board;
        for c in 0..5 {
            board.set_gem(Pos::new(4, c), Some(Gem::plain(if c == 2 { 3 } else { 1 })));
        }
        board.set_gem(Pos::new(3, 2), Some(Gem::plain(1)));
        assert!(session.game_mut().try_swap(Pos::new(3, 2), Pos::new(4, 2)));
        for _ in 0..400 {
            session.update(16.0);
            if session.game().phase() == Phase::Idle {
                break;
            }
        }
        assert_eq!(
            session.game().swap_match(),
            5,
            "the board was laid out for a five and did not make one",
        );
        assert_eq!(matches(&session), [5], "a five paid the wrong sizes");
    }

    #[test]
    fn a_level_remembers_the_best_score_it_was_beaten_with() {
        let mut session = run_finding_no_unlock();
        assert_eq!(session.best_score(0), 0, "a run that has played nothing has a best score");

        // At least the pin rather than exactly it: the helper holds the score
        // there every frame while the flourish keeps adding to it, so the
        // best is taken a hair above what it was pinned at. Which is the
        // behavior wanted, since the score really does climb through the
        // flourish; the gaps below are wide enough that it cannot matter.
        force_win_at(&mut session, 9_000);
        let first = session.best_score(0);
        assert!(first >= 9_000, "a level beaten at 9,000 kept {first}");

        // A worse attempt does not take it away: the number belongs to the
        // level, not to the last go at it, which is the same rule the marks
        // follow.
        session.retry();
        force_win_at(&mut session, 4_000);
        assert_eq!(session.best_score(0), first, "a worse attempt lowered the best");

        session.retry();
        force_win_at(&mut session, 21_000);
        assert!(
            session.best_score(0) > first,
            "a better attempt did not raise it past {first}",
        );

        // Per level, and only where it was earned.
        assert_eq!(session.best_score(1), 0, "a level never played has a score");
        assert_eq!(session.best_score(999), 0, "a level that does not exist has a score");
    }

    #[test]
    fn a_lost_level_scores_nothing_it_can_keep() {
        // The best score is what a level was *beaten* with, which is the same
        // question the marks answer. An attempt that ran out of moves scored
        // points, and none of them are what any of this is about.
        let mut session = run_finding_no_unlock();
        session.game_mut().progress.score = 50_000;
        for _ in 0..4000 {
            if session.game().phase() == Phase::Finished {
                break;
            }
            if let Some((a, b)) = first_move(&mut session) {
                session.game_mut().try_swap(a, b);
            }
            session.update(16.0);
        }
        assert_eq!(session.game().status(), Status::Lost, "this run was supposed to lose");
        assert_eq!(session.best_score(0), 0, "a level that was never beaten kept a score");
    }

    #[test]
    fn a_restored_best_score_comes_back_and_cannot_be_talked_down() {
        let mut session = Session::new(7);
        session.restore_best_score(2, 31_000);
        assert_eq!(session.best_score(2), 31_000);

        // A save cannot take away what the run has already done, and reading
        // the same save twice cannot lower it.
        session.restore_best_score(2, 12_000);
        assert_eq!(session.best_score(2), 31_000, "a stale save lowered a best score");
        // Nor can it reach past the end of the ladder.
        session.restore_best_score(999, 5);
        assert_eq!(session.best_score(999), 0);
    }

    #[test]
    fn a_score_past_a_tier_checks_it_and_a_score_short_of_one_does_not() {
        let marks = |session: &Session| (session.level().silver, session.level().gold);

        // A run that finds no unlock by clearing, so the flourish has nothing
        // to mint and the pinned score is the score the marks are read off.
        // One that found an unlock would score on top of it and sail past the
        // mark this is checking it stays short of.
        let mut session = run_finding_no_unlock();
        let (silver, gold) = marks(&session);
        assert!(silver > 0 && gold > silver, "the opener has no tiers to test");
        force_win_at(&mut session, silver - 1);
        assert!(session.checked().contains(&Location::LevelClear(0).id()));
        assert!(
            !session.checked().contains(&Location::LevelSilver(0).id()),
            "a score short of silver checked it anyway",
        );

        // Past silver but short of gold.
        let mut session = run_finding_no_unlock();
        force_win_at(&mut session, gold - 1);
        assert!(session.checked().contains(&Location::LevelSilver(0).id()), "silver was missed");
        assert!(
            !session.checked().contains(&Location::LevelGold(0).id()),
            "a score short of gold checked it anyway",
        );

        // Past both. Exactly on the mark counts as reaching it.
        let mut session = run_finding_no_unlock();
        force_win_at(&mut session, gold);
        assert!(session.checked().contains(&Location::LevelSilver(0).id()));
        assert!(session.checked().contains(&Location::LevelGold(0).id()), "gold was missed");
    }

    #[test]
    fn an_unlock_found_by_clearing_is_there_for_the_flourish_it_paid_for() {
        // The flourish turns leftover moves into specials, and which specials
        // it may mint is what the run holds. An unlock handed over for
        // clearing the level has to be in hand for the rest of that flourish,
        // or the one thing it could have changed has already happened by the
        // time it arrives.
        let (mut session, special) = run_finding_an_unlock();
        assert!(
            !has(session.game().rules().specials, special),
            "the run was already holding what it is meant to find",
        );

        let mut during_flourish = None;
        session.game_mut().progress.score = 1_000_000;
        while session.game().phase() != Phase::Finished {
            meet_everything_but_the_score(session.game_mut());
            if let Some((a, b)) = first_move(&mut session) {
                session.game_mut().try_swap(a, b);
            }
            session.update(16.0);
            if during_flourish.is_none()
                && matches!(session.game().phase(), Phase::CashingIn { .. })
            {
                during_flourish = Some(session.game().rules().specials);
            }
        }
        let during_flourish = during_flourish.expect("the level never cashed in its moves");
        assert!(
            has(during_flourish, special),
            "the board could not make a {special:?} until after the flourish that paid for it",
        );
    }

    /// Whether a set allows one particular special.
    fn has(set: SpecialSet, special: Special) -> bool {
        set.list().contains(&special)
    }

    #[test]
    fn a_mark_crossed_during_the_flourish_pays_while_it_is_still_running() {
        // Same reason, from the other end: the score climbs all the way
        // through the flourish, so a mark can be crossed in there, and what
        // crossing it pays is only worth anything while there is still
        // flourish left to spend it on. Waiting for the board to stop would
        // hand it over one moment too late.
        let (mut session, _) = run_finding_an_unlock();
        let silver = session.level().silver;
        assert!(silver > 0, "the opener has no silver mark to cross");

        let mut paid_while_playing = false;
        let mut at_clear = None;
        while session.game().phase() != Phase::Finished {
            // Held just under silver until the level clears, and let go the
            // moment it does. A board played out honestly can cross the mark
            // before it clears, and then there is nothing left for the
            // flourish to carry it over.
            if !session.game().cleared() {
                session.game_mut().progress.score = silver - 500;
                meet_everything_but_the_score(session.game_mut());
            }
            if let Some((a, b)) = first_move(&mut session) {
                session.game_mut().try_swap(a, b);
            }
            session.update(16.0);
            if at_clear.is_none() && session.game().cleared() {
                at_clear = Some(session.game().progress.score);
            }
            if session.game().status() == Status::Playing
                && session.checked().contains(&Location::LevelSilver(0).id())
            {
                paid_while_playing = true;
            }
        }

        assert!(
            at_clear.expect("the level never cleared") < silver,
            "it was already past silver when the level cleared, so this proves nothing",
        );
        assert!(
            session.checked().contains(&Location::LevelSilver(0).id()),
            "the flourish did not carry it over silver, so this proves nothing",
        );
        assert!(
            paid_while_playing,
            "silver waited for the board to stop before it paid",
        );
    }

    #[test]
    fn a_tier_is_judged_on_the_score_the_flourish_finished_on() {
        // The flourish keeps adding right up to the end, so a tier read
        // before it would be read off a smaller number than the player sees.
        // Run holding an unlock, because with none there is nothing to mint
        // and so nothing for the flourish to add.
        // A pinned seed, because what this needs is a board whose score at the
        // goal sits just under gold. Re-pin by sweeping seeds for one where
        // the goal is met below the mark and the flourish carries it over.
        let mut session = Session::new(1);
        session.receive(Item::Unlock(Special::LineH));
        let gold = session.level().gold;
        session.game_mut().progress.score = gold - 400;

        // Watched rather than inflated, and the score at the moment the goal
        // was met is what makes this mean anything: if it were already past
        // gold there, judging it early would look the same as judging it late.
        let mut at_goal = None;
        while session.game().phase() != Phase::Finished {
            if !session.game().cleared() {
                meet_everything_but_the_score(session.game_mut());
            }
            if let Some((a, b)) = first_move(&mut session) {
                session.game_mut().try_swap(a, b);
            }
            session.update(16.0);
            let said_cleared =
                session.game().events().iter().any(|e| e.kind == crate::game::EV_CLEARED);
            if at_goal.is_none() && said_cleared {
                at_goal = Some(session.game().progress.score);
            }
        }
        let at_goal = at_goal.expect("the level never said it was cleared");

        assert!(at_goal < gold, "it was already past gold at the goal, so this proves nothing");
        assert!(
            session.game().progress.score >= gold,
            "the flourish did not carry it over, so this proves nothing",
        );
        assert!(
            session.checked().contains(&Location::LevelGold(0).id()),
            "gold was judged at the goal rather than once the flourish had paid",
        );
    }

    /// Collects one Archipelago gem on the board in front of the session, by
    /// putting one where a match is about to happen.
    fn collect_one_gem(session: &mut Session) {
        let board = &mut session.game_mut().board;
        for c in 0..3 {
            board.set_gem(Pos::new(1, c), Some(Gem::plain(1)));
        }
        board.set_gem(Pos::new(1, 2), Some(Gem::plain(2)));
        board.set_gem(Pos::new(2, 2), Some(Gem::plain(1)));
        board.set_gem(Pos::new(0, 0), Some(Gem::archipelago()));
        let (a, b) = (Pos::new(1, 2), Pos::new(2, 2));
        assert!(session.game_mut().try_swap(a, b), "the setup swap was refused");
        for _ in 0..400 {
            session.update(16.0);
            if session.game().phase() == Phase::Idle {
                break;
            }
        }
    }

    #[test]
    fn collecting_a_gem_checks_the_next_in_the_levels_sequence() {
        // Which gem was cleared on the board says nothing about which check it
        // pays. They go in order, and the order belongs to the run: coming
        // back to a level and clearing one takes the next it has not had, not
        // the first.
        let mut session = Session::set_up(7, Options { ap_gems: 3, ..Options::default() });
        assert!(session.checked().is_empty());

        collect_one_gem(&mut session);
        // The gems it checked rather than everything it checked: the move
        // that clears a gem is a match and may set off a chain, and both of
        // those are locations too.
        let gems: Vec<u32> = session
            .checked()
            .iter()
            .copied()
            .filter(|id| matches!(Location::from_id(*id), Some(Location::ApGem { .. })))
            .collect();
        assert_eq!(
            gems,
            [Location::ApGem { level: 0, index: 0 }.id()],
            "the first gem collected did not check the first of the sequence",
        );

        // A fresh board on the same level, as a retry deals. The sequence
        // carries over rather than starting again.
        session.retry();
        collect_one_gem(&mut session);
        assert!(
            session.checked().contains(&Location::ApGem { level: 0, index: 1 }.id()),
            "a second playthrough checked the first gem again instead of the next",
        );
        // Counted rather than taking the length of everything checked, since
        // a match that clears a gem can set off a chain, and a chain is a
        // location too.
        let gems = session
            .checked()
            .iter()
            .filter(|id| matches!(Location::from_id(**id), Some(Location::ApGem { .. })))
            .count();
        assert_eq!(gems, 2, "two gems collected paid for {gems} checks");
    }

    #[test]
    fn a_level_stops_dealing_gems_once_its_checks_are_taken() {
        // The board is told how many are still worth spawning, so a level
        // whose run is exhausted deals none. Without it a player would keep
        // clearing gems that pay nothing.
        let mut session = Session::set_up(7, Options { ap_gems: 1, ..Options::default() });
        assert_eq!(session.game().ap_gems_wanted, 1, "the opening level should want its one");

        collect_one_gem(&mut session);
        assert!(session.checked().contains(&Location::ApGem { level: 0, index: 0 }.id()));

        session.retry();
        assert_eq!(
            session.game().ap_gems_wanted,
            0,
            "the level still wants gems after its only check was taken",
        );
    }

    #[test]
    fn a_level_drops_no_more_gems_than_it_has_checks_for() {
        // Not just no more at once: no more over the whole playthrough. A gem
        // that falls after the level's last check is taken is one the player
        // clears for nothing, which reads as the check being broken.
        // At odds the yaml does not offer, so this cannot pass by a second gem
        // simply not happening to fall: one refilled gem in two is one of
        // these, and a level refills hundreds.
        let mut session =
            Session::set_up(7, Options { ap_gems: 1, ap_gem_odds: 2, ..Options::default() });
        collect_one_gem(&mut session);
        assert!(session.checked().contains(&Location::ApGem { level: 0, index: 0 }.id()));

        // Play the rest of the level out. The one check is gone, so nothing
        // more should ever fall here.
        for _ in 0..4000 {
            if session.game().status() != Status::Playing {
                break;
            }
            if session.game().accepts_input() {
                match first_move(&mut session) {
                    Some((a, b)) => {
                        session.game_mut().try_swap(a, b);
                    }
                    None => break,
                }
            }
            session.update(16.0);
            assert!(
                session.game().board.ap_gems().is_empty(),
                "a gem fell after the level's only check had been taken",
            );
        }
    }

    #[test]
    fn a_run_asking_for_no_gems_is_dealt_none() {
        let mut session = Session::set_up(7, Options { ap_gems: 0, ..Options::default() });
        assert_eq!(session.game().ap_gems_wanted, 0);
        for _ in 0..600 {
            session.update(16.0);
            if let Some((a, b)) = first_move(&mut session) {
                session.game_mut().try_swap(a, b);
            }
            assert!(
                session.game().board.ap_gems().is_empty(),
                "a run that asked for no gems was dealt one",
            );
        }
    }

    #[test]
    fn a_chain_checks_its_location_and_pays_once() {
        // Two deep turns up by itself soon enough: a clear that drops gems
        // into another match. Pinned, because "soon enough" is only true of
        // most boards and this one has three moves to do it in.
        let mut session = Session::new(0);
        let chain = Location::Chain(SHORTEST_CHAIN).id();
        for _ in 0..4000 {
            if session.checked().contains(&chain) {
                break;
            }
            if session.game().phase() == Phase::Idle && session.game().status() == Status::Playing {
                if let Some((a, b)) = first_move(&mut session) {
                    session.game_mut().try_swap(a, b);
                }
            }
            session.update(16.0);
        }
        assert!(session.checked().contains(&chain), "no chain was ever registered");
        if let Some(held) = session.holds(Location::Chain(SHORTEST_CHAIN)) {
            assert!(session.inventory().has(held), "the chain did not pay what it holds");
        }

        // And it is spent. Playing on, chains of two keep happening, and none
        // of them is worth anything a second time. Counted as visits to the
        // location rather than as items, because other locations on this same
        // level hand over moves for it too.
        for _ in 0..2000 {
            if session.game().phase() == Phase::Idle && session.game().status() == Status::Playing {
                if let Some((a, b)) = first_move(&mut session) {
                    session.game_mut().try_swap(a, b);
                }
            }
            session.update(16.0);
        }
        let visits = session.checked().iter().filter(|id| **id == chain).count();
        assert_eq!(visits, 1, "the location was checked {visits} times");
    }

    #[test]
    fn a_run_finds_bonus_items_and_they_turn_up_in_what_it_is_carrying() {
        // The loop the player sees: the pool holds some, the fill puts them
        // somewhere, and checking that somewhere leaves the bar along the
        // bottom with something in it. Every link of that is tested on its
        // own; this is the one that says they are joined up.
        let mut session = Session::new(7);
        assert!(
            CONSUMABLES.iter().all(|kind| session.consumables(*kind) == 0),
            "a run opened already carrying something to spend",
        );

        let holding: Vec<(Location, Consumable)> = locations(session.level_count())
            .into_iter()
            .filter_map(|at| match session.holds(at) {
                Some(Item::Consumable(kind)) => Some((at, kind)),
                _ => None,
            })
            .collect();
        assert!(
            !holding.is_empty(),
            "a run set up the usual way hid no bonus items anywhere in the world",
        );

        let mut owed = [0u32; CONSUMABLES.len()];
        for (at, kind) in &holding {
            owed[kind.code() as usize] += 1;
            session.restore(at.id());
        }
        for kind in CONSUMABLES {
            assert_eq!(
                session.consumables(kind),
                owed[kind.code() as usize],
                "the run found {} of {kind:?} and is carrying {}",
                owed[kind.code() as usize],
                session.consumables(kind),
            );
        }
    }

    #[test]
    fn a_restored_location_rebuilds_what_it_gave_without_announcing_it() {
        // Coming back to a run has to leave it holding what it held. Replaying
        // the finds through the feed would be a wall of news about nothing.
        // Two locations that hold something, whatever the fill put there.
        let mut session = Session::new(7);
        let found: Vec<(Location, Item)> = locations(session.level_count())
            .into_iter()
            .filter_map(|at| session.holds(at).map(|item| (at, item)))
            .take(2)
            .collect();
        assert_eq!(found.len(), 2, "the placement is too empty to test with");

        for (at, _) in &found {
            session.restore(at.id());
        }
        for (at, item) in &found {
            assert!(
                session.inventory().has(*item),
                "{} did not give back {}",
                location_name(*at),
                item_name(*item),
            );
        }
        assert!(session.events().is_empty(), "restoring announced itself");

        // And the board in play is what the restored run may do.
        let mut expected = session.level().clone();
        session.inventory().apply(session.index(), &mut expected);
        assert_eq!(session.game().rules().specials, expected.rules.specials);
    }

    #[test]
    fn a_restored_location_cannot_be_found_again() {
        let mut session = Session::new(7);
        let at = locations(session.level_count())
            .into_iter()
            .find(|at| matches!(session.holds(*at), Some(Item::Moves { .. })))
            .expect("something in the placement stacks");
        let Some(Item::Moves { level }) = session.holds(at) else { unreachable!() };

        session.restore(at.id());
        session.restore(at.id());
        assert_eq!(session.inventory().moves_found(level), 1, "restoring twice paid twice");
        assert_eq!(session.checked(), [at.id()], "and it was written down twice");
    }

    /// What the level select's two status marks are read from.
    #[test]
    fn a_level_says_how_many_of_its_gems_have_been_taken() {
        let mut session = Session::new(7);
        let wanted = session.gems_per_level();
        assert!(wanted > 0, "a run with no gems in it proves nothing here");
        assert_eq!(session.gems_found(2), 0, "a fresh run has taken one");

        session.restore(Location::ApGem { level: 2, index: 0 }.id());
        assert_eq!(session.gems_found(2), 1);
        assert_eq!(session.gems_found(3), 0, "a gem was counted against the wrong level");

        for index in 0..wanted {
            session.restore(Location::ApGem { level: 2, index }.id());
        }
        assert_eq!(session.gems_found(2), wanted, "the level never reads as finished");
    }

    #[test]
    fn a_level_says_whether_its_moves_upgrade_has_turned_up() {
        let mut session = Session::new(7);
        // One per level today, which is why the mark is found or not found
        // rather than a fraction. It fills by thirds the day the pool carries
        // three of them, which is what reading the total out of the pool is
        // for.
        assert_eq!(session.moves_total(3), 1, "a level carries some other number of upgrades now");
        assert_eq!(session.moves_found(3), 0);

        assert!(session.receive(Item::Moves { level: 3 }));
        assert_eq!(session.moves_found(3), 1, "the upgrade did not read as found");
        assert_eq!(session.moves_found(4), 0, "it was counted against the wrong level");
    }

    #[test]
    fn a_save_from_somewhere_else_is_ignored_rather_than_believed() {
        let mut session = Session::new(7);
        session.restore(9_999);
        session.restore(Location::LevelClear(session.level_count() + 5).id());
        assert!(session.checked().is_empty(), "an id from nowhere was taken as a location");
        assert!(session.inventory().specials().is_empty());
    }

    #[test]
    fn moves_arriving_during_a_level_go_on_the_counter_in_front_of_the_player() {
        // What a multiworld delivering mid level looks like. Waiting until the
        // next deal would mean an item that does nothing for the level it was
        // sent to.
        let mut session = Session::new(7);
        let before = session.game().moves_left;
        let budget = session.game().spec.moves;

        let upgrade = session.game().spec.moves_upgrade;
        assert!(upgrade > 0, "the opening level grants nothing, so this proves nothing");
        assert!(session.receive(Item::Moves { level: 0 }));
        assert_eq!(session.game().moves_left, before + upgrade);
        assert_eq!(session.game().spec.moves, budget + upgrade, "and the budget grew with it");
    }

    #[test]
    fn an_item_from_outside_the_ladder_reaches_the_board_in_play() {
        // How the multiworld delivers. An unlock arriving during a level has
        // to open up that level, not the one after it.
        let mut session = Session::new(7);
        assert!(session.game().rules().specials.is_empty());

        assert!(session.receive(Item::Unlock(Special::Rocket)));
        assert_eq!(
            session.game().rules().specials,
            SpecialSet { rocket: true, ..SpecialSet::NONE },
            "the board in front of the player did not open up",
        );
        assert!(!session.receive(Item::Unlock(Special::Rocket)), "the second is not news");
    }

    #[test]
    fn a_new_session_starts_on_the_first_level_only() {
        let session = Session::new(7);
        assert_eq!(session.index(), 0);
        assert_eq!(session.unlocked(), 1);
        assert_eq!(session.level_name(), b"First Light");
    }

    #[test]
    fn the_whole_ladder_is_readable_for_a_level_picker() {
        let session = Session::new(7);
        let blob = String::from_utf8(session.level_names().to_vec()).unwrap();
        let names: Vec<&str> = blob.split('\n').collect();
        assert_eq!(names.len(), session.level_count());
        assert_eq!(names[0], "First Light");
        assert!(names.iter().all(|name| !name.is_empty()));
    }

    #[test]
    fn locked_levels_refuse_to_load() {
        let mut session = Session::new(7);
        assert!(!session.load(3), "level 4 has not been earned");
        assert!(!session.load(999));
        assert_eq!(session.index(), 0);
        assert!(session.load(0), "the current level can always be reloaded");
    }

    #[test]
    fn winning_unlocks_the_next_level() {
        let mut session = climbing(7);
        force_win(&mut session);
        assert_eq!(session.unlocked(), 2);
        assert!(session.next_level());
        assert_eq!(session.index(), 1);
        assert_eq!(session.level_name(), b"Finding Fours");
    }

    #[test]
    fn the_ladder_ends_cleanly() {
        let mut session = climbing(7);
        let last = session.level_count() - 1;
        session.unlocked = session.level_count();
        assert!(session.load(last));
        assert!(!session.next_level(), "there is nothing after the last level");
        assert_eq!(session.index(), last);
    }

    /// The whole of what the setting changes, from the player's side.
    #[test]
    fn a_ladder_that_opens_by_item_does_not_open_by_clearing() {
        let mut options = Options::default();
        options.progressive_levels = 1;
        let mut session = Session::set_up(7, options);
        assert_eq!(session.unlocked(), 1, "a fresh run opened more than the first level");

        // Clearing the level it is on opens nothing, which is the difference.
        // Said to the session the way winning says it, since what is being
        // checked is that the count does not follow the win.
        session.unlocked = 9;
        assert_eq!(session.unlocked(), 1, "a clear opened a level the run was not handed");
        assert!(!session.load(1), "a level nobody has been handed can be played");

        // The item is what opens them, one each, in order.
        assert!(session.receive(Item::LevelUnlock));
        assert_eq!(session.unlocked(), 2);
        assert!(session.load(1), "the level the unlock opened cannot be played");
        assert!(!session.load(2), "one unlock opened two levels");

        assert!(session.receive(Item::LevelUnlock));
        assert_eq!(session.unlocked(), 3);

        // And the spares stop at the top of the ladder rather than counting
        // past it.
        for _ in 0..session.level_count() + 5 {
            session.receive(Item::LevelUnlock);
        }
        assert_eq!(session.unlocked(), session.level_count());
    }

    #[test]
    fn a_ladder_that_opens_by_clearing_ignores_the_item() {
        // The other way round: the count a clear keeps is the answer, and an
        // unlock arriving from somewhere changes nothing. It cannot arrive in
        // a solo run set up this way, because none are in the pool, but a
        // multiworld can send anything.
        let mut session = climbing(7);
        assert_eq!(session.unlocked(), 1);
        session.receive(Item::LevelUnlock);
        assert_eq!(session.unlocked(), 1, "an unlock opened a level in a run not using them");
        session.unlocked = 3;
        assert_eq!(session.unlocked(), 3);
    }

    #[test]
    fn levels_are_reproducible_but_not_identical() {
        let a = climbing(99);
        let b = climbing(99);
        assert!(a
            .game()
            .board
            .positions()
            .all(|p| a.game().board.gem(p) == b.game().board.gem(p)));

        let mut c = climbing(99);
        c.unlocked = c.level_count();
        c.load(1);
        let differs = c
            .game()
            .board
            .positions()
            .any(|p| c.game().board.gem(p) != a.game().board.gem(p));
        assert!(differs, "each level should deal its own board");
    }

    #[test]
    fn restoring_progress_is_clamped_to_the_ladder() {
        // A run whose ladder opens by clearing, because this is about the
        // count a save hands back, and a run opening it by item has no such
        // count: what it holds is the answer. That is
        // `a_ladder_that_opens_by_item_does_not_open_by_clearing`.
        let mut session = climbing(7);
        session.set_unlocked(4);
        assert_eq!(session.unlocked(), 4);
        assert!(session.load(3));
        session.set_unlocked(0);
        assert_eq!(session.unlocked(), 4, "restoring never takes a level away");
        session.set_unlocked(9_999);
        assert_eq!(session.unlocked(), session.level_count());
    }

    /// Every gem on the board, for comparing one deal against another.
    fn deal_of(session: &Session) -> Vec<Option<crate::board::Gem>> {
        let board = &session.game().board;
        board.positions().map(|p| board.gem(p)).collect()
    }

    #[test]
    fn another_go_at_a_level_deals_a_different_board() {
        // A level that laid out badly should be one restart away from a better
        // board rather than something to grind against.
        let mut session = Session::new(7);
        let before = deal_of(&session);
        session.retry();
        assert_ne!(before, deal_of(&session), "retrying replayed the same deal");
    }

    #[test]
    fn a_run_walked_the_same_way_deals_the_same_boards() {
        // Rerolling is per level start, not per call: a whole run still
        // follows from its seed, which is what the screenshot tooling and the
        // difficulty bots rely on.
        let walk = |seed| {
            let mut session = climbing(seed);
            session.retry();
            session.set_unlocked(3);
            assert!(session.load(2));
            deal_of(&session)
        };
        assert_eq!(walk(42), walk(42), "the same run dealt two different sets of boards");
        assert_ne!(walk(42), walk(43), "two runs dealt the same boards");
    }

    #[test]
    fn retry_restores_the_level_without_unlocking_anything() {
        let mut session = climbing(7);
        let (a, b) = first_move(&mut session).unwrap();
        session.game_mut().try_swap(a, b);
        for _ in 0..200 {
            session.update(16.0);
        }
        session.retry();
        assert_eq!(session.game().progress.score, 0);
        assert_eq!(session.unlocked(), 1);
    }
}
