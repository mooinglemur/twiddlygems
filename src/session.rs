//! A run through the level ladder.
//!
//! [`Session`] is the seam where progression lives: today it unlocks the next
//! level when you clear one, and later the Archipelago layer will answer the
//! same question (which levels may be played) from received items instead.

use crate::game::{Event, Game, Status, EV_ITEM};
use crate::level::{levels, LevelSpec};
use crate::progression::{
    fill_seed, item_index, item_name, items, location_index, location_name, locations,
    solo_placement, Inventory, Item, Location, Tier, LONGEST_CHAIN, NO_LOCATION, SHORTEST_CHAIN,
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
    /// Things that happened to the run rather than to the board, raised
    /// alongside the board's own so the page has one stream to watch.
    events: Vec<Event>,
    game: Game,
    name_buf: Vec<u8>,
    names_blob: Vec<u8>,
    item_names_blob: Vec<u8>,
    location_names_blob: Vec<u8>,
}

/// Joins names into the newline separated blob the ABI hands over.
fn blob(names: impl Iterator<Item = String>) -> Vec<u8> {
    names.collect::<Vec<_>>().join("\n").into_bytes()
}

impl Session {
    /// Opens a run. The seed is the whole run: it deals every board, and it
    /// deals the progression, so two players on the same seed play the same
    /// game and nobody else plays theirs.
    pub fn new(seed: u64) -> Self {
        let levels = levels();
        let first = levels.first().expect("the ladder is never empty");
        let mut deal = Rng::new(seed);
        // A run starts holding nothing, so the opener plays as plain matching
        // however much its own rules would otherwise allow.
        let inventory = Inventory::empty();
        let mut session = Session {
            game: open_level(0, first, &inventory, deal.next_u64()),
            levels: levels.clone(),
            index: 0,
            unlocked: 1,
            deal,
            inventory,
            checked: Vec::new(),
            placement: solo_placement(levels.len(), fill_seed(seed)),
            events: Vec::new(),
            name_buf: Vec::new(),
            names_blob: Vec::new(),
            item_names_blob: Vec::new(),
            location_names_blob: Vec::new(),
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

    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn unlocked(&self) -> usize {
        self.unlocked
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

    /// Every location's name, in the order [`locations`] gives them.
    pub fn location_names(&self) -> &[u8] {
        &self.location_names_blob
    }

    /// Switches to a level the player has unlocked. Returns false for one they
    /// have not, or for an index off the end of the ladder.
    pub fn load(&mut self, index: usize) -> bool {
        if index >= self.levels.len() || index >= self.unlocked {
            return false;
        }
        self.index = index;
        self.deal_level();
        true
    }

    /// Moves on after a win. Returns false when the ladder is finished.
    pub fn next_level(&mut self) -> bool {
        let next = self.index + 1;
        if next >= self.levels.len() {
            return false;
        }
        self.unlocked = self.unlocked.max(next + 1);
        self.load(next)
    }

    /// Restores how far a returning player had got. Clamped to the ladder, and
    /// never used to lock something already unlocked this run.
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
        self.game = open_level(self.index, &self.levels[self.index], &self.inventory, seed);
        self.sync_name();
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
        match location {
            Location::LevelClear(index)
            | Location::LevelSilver(index)
            | Location::LevelGold(index)
                if index >= self.levels.len() =>
            {
                return
            }
            _ => {}
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
    }

    pub fn update(&mut self, dt_ms: f32) {
        // Cleared here rather than drained by the reader, to match how the
        // board reports: everything raised during this call, and nothing else.
        self.events.clear();
        self.game.update(dt_ms);
        if self.game.status() == Status::Won {
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
fn open_level(index: usize, spec: &LevelSpec, inventory: &Inventory, seed: u64) -> Game {
    let mut spec = spec.clone();
    inventory.apply(index, &mut spec);
    Game::new(spec, seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Special;
    use crate::game::Phase;
    use crate::rules::SpecialSet;

    /// Wins the level in front of the session without playing it properly.
    ///
    /// The score is put out of reach and then the board is nudged until it
    /// ends, because what these tests are about is what a clear leads to, not
    /// whether the level is beatable.
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
            session.game_mut().progress.score = score;
            if let Some((a, b)) = session.game().hint() {
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
        for seed in 0..64 {
            let session = Session::new(seed);
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
            let session = Session::new(seed);
            if let Some(Item::Moves { .. }) = session.holds(Location::LevelClear(0)) {
                return session;
            }
        }
        panic!("no run in the first 64 seeds pays only moves for the opening level");
    }

    fn force_win(session: &mut Session) -> Vec<Announced> {
        let mut seen = Vec::new();
        session.game_mut().progress.score = 1_000_000;
        while session.game().phase() != Phase::Finished {
            if let Some((a, b)) = session.game().hint() {
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
            if let Some((a, b)) = session.game().hint() {
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
            }
            if let Some((a, b)) = session.game().hint() {
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
        let mut session = Session::new(7);
        session.receive(Item::Unlock(Special::LineH));
        let gold = session.level().gold;
        session.game_mut().progress.score = gold - 400;

        // Watched rather than inflated, and the score at the moment the goal
        // was met is what makes this mean anything: if it were already past
        // gold there, judging it early would look the same as judging it late.
        let mut at_goal = None;
        while session.game().phase() != Phase::Finished {
            if let Some((a, b)) = session.game().hint() {
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

    #[test]
    fn a_chain_checks_its_location_and_pays_once() {
        // Two deep turns up by itself soon enough: a clear that drops gems
        // into another match.
        let mut session = Session::new(11);
        let chain = Location::Chain(SHORTEST_CHAIN).id();
        for _ in 0..4000 {
            if session.checked().contains(&chain) {
                break;
            }
            if session.game().phase() == Phase::Idle && session.game().status() == Status::Playing {
                if let Some((a, b)) = session.game().hint() {
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
                if let Some((a, b)) = session.game().hint() {
                    session.game_mut().try_swap(a, b);
                }
            }
            session.update(16.0);
        }
        let visits = session.checked().iter().filter(|id| **id == chain).count();
        assert_eq!(visits, 1, "the location was checked {visits} times");
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

        assert!(session.receive(Item::Moves { level: 0 }));
        let step = crate::progression::move_step(budget);
        assert_eq!(session.game().moves_left, before + step);
        assert_eq!(session.game().spec.moves, budget + step, "and the budget grew with it");
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
        let mut session = Session::new(7);
        force_win(&mut session);
        assert_eq!(session.unlocked(), 2);
        assert!(session.next_level());
        assert_eq!(session.index(), 1);
        assert_eq!(session.level_name(), b"Finding Fours");
    }

    #[test]
    fn the_ladder_ends_cleanly() {
        let mut session = Session::new(7);
        let last = session.level_count() - 1;
        session.unlocked = session.level_count();
        assert!(session.load(last));
        assert!(!session.next_level(), "there is nothing after the last level");
        assert_eq!(session.index(), last);
    }

    #[test]
    fn levels_are_reproducible_but_not_identical() {
        let a = Session::new(99);
        let b = Session::new(99);
        assert!(a
            .game()
            .board
            .positions()
            .all(|p| a.game().board.gem(p) == b.game().board.gem(p)));

        let mut c = Session::new(99);
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
        let mut session = Session::new(7);
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
            let mut session = Session::new(seed);
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
        let mut session = Session::new(7);
        let (a, b) = session.game().hint().unwrap();
        session.game_mut().try_swap(a, b);
        for _ in 0..200 {
            session.update(16.0);
        }
        session.retry();
        assert_eq!(session.game().progress.score, 0);
        assert_eq!(session.unlocked(), 1);
    }
}
