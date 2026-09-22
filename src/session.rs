//! A run through the level ladder.
//!
//! [`Session`] is the seam where progression lives: today it unlocks the next
//! level when you clear one, and later the Archipelago layer will answer the
//! same question (which levels may be played) from received items instead.

use crate::game::{Event, Game, Status, EV_ITEM};
use crate::level::{levels, LevelSpec};
use crate::progression::{solo_grant, Inventory, Item};
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
    /// The item the level just cleared handed over, if it was new. Read by the
    /// front end to say so, and cleared the moment another level is dealt.
    granted: Option<Item>,
    /// Things that happened to the run rather than to the board, raised
    /// alongside the board's own so the page has one stream to watch.
    events: Vec<Event>,
    game: Game,
    name_buf: Vec<u8>,
    names_blob: Vec<u8>,
}

impl Session {
    pub fn new(seed: u64) -> Self {
        let levels = levels();
        let first = levels.first().expect("the ladder is never empty");
        let mut deal = Rng::new(seed);
        // A run starts holding nothing, so the opener plays as plain matching
        // however much its own rules would otherwise allow.
        let inventory = Inventory::empty();
        let mut session = Session {
            game: open_level(first, &inventory, deal.next_u64()),
            levels: levels.clone(),
            index: 0,
            unlocked: 1,
            deal,
            inventory,
            granted: None,
            events: Vec::new(),
            name_buf: Vec::new(),
            names_blob: Vec::new(),
        };
        session.names_blob = session
            .levels
            .iter()
            .map(|level| level.name)
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
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
        self.game = open_level(&self.levels[self.index], &self.inventory, seed);
        self.granted = None;
        self.sync_name();
    }

    /// What the run may make. The front end shows it; the Archipelago layer
    /// will answer the same question from received items.
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// What clearing the current level handed over, if it was something new.
    pub fn granted(&self) -> Option<Item> {
        self.granted
    }

    /// Takes an item in from outside the ladder, which is how the multiworld
    /// will deliver. Returns whether it was new.
    ///
    /// It reaches the board in front of the player rather than waiting for the
    /// next deal: an unlock that arrives mid level should be usable in that
    /// level, which is what receiving it means.
    pub fn receive(&mut self, item: Item) -> bool {
        let is_new = self.inventory.receive(item);
        if is_new {
            self.refresh_specials();
            self.events.push(Event::about_item(EV_ITEM, item.kind(), item.value()));
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
        let mut rules = self.levels[self.index].rules;
        self.inventory.apply(&mut rules);
        self.game.spec.rules.specials = rules.specials;
    }

    /// Collects what the level just cleared is holding.
    ///
    /// Idempotent, because it runs on every frame the board sits won: taking
    /// the same unlock again changes nothing, and only the first time counts
    /// as news.
    fn collect_clear_reward(&mut self) {
        if let Some(item) = solo_grant(self.index) {
            if self.receive(item) {
                self.granted = Some(item);
            }
        }
    }

    pub fn update(&mut self, dt_ms: f32) {
        // Cleared here rather than drained by the reader, to match how the
        // board reports: everything raised during this call, and nothing else.
        self.events.clear();
        self.game.update(dt_ms);
        if self.game.status() == Status::Won {
            self.unlocked = self.unlocked.max((self.index + 2).min(self.levels.len()));
            self.collect_clear_reward();
        }
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
fn open_level(spec: &LevelSpec, inventory: &Inventory, seed: u64) -> Game {
    let mut spec = spec.clone();
    inventory.apply(&mut spec.rules);
    Game::new(spec, seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Special;
    use crate::game::Phase;
    use crate::progression::UNLOCKS;
    use crate::rules::SpecialSet;

    /// Wins the level in front of the session without playing it properly.
    ///
    /// The score is put out of reach and then the board is nudged until it
    /// ends, because what these tests are about is what a clear leads to, not
    /// whether the level is beatable.
    fn force_win(session: &mut Session) {
        session.game_mut().progress.score = 1_000_000;
        while session.game().phase() != Phase::Finished {
            if let Some((a, b)) = session.game().hint() {
                session.game_mut().try_swap(a, b);
            }
            session.update(16.0);
        }
        assert_eq!(session.game().status(), Status::Won);
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
        assert_eq!(session.granted(), None);
    }

    #[test]
    fn clearing_a_level_hands_over_an_unlock_the_next_one_can_use() {
        let mut session = Session::new(7);
        force_win(&mut session);
        assert_eq!(
            session.granted(),
            Some(Item::Unlock(UNLOCKS[0])),
            "clearing the opener should hand over the first of the pool",
        );

        assert!(session.next_level());
        assert_eq!(session.granted(), None, "the news is cleared once it has been acted on");
        assert_eq!(
            session.game().rules().specials,
            SpecialSet { line_v: true, ..SpecialSet::NONE },
            "and the level after it is played holding exactly that",
        );
    }

    #[test]
    fn a_cleared_level_hands_over_its_unlock_only_the_first_time() {
        // `update` sees a won board on every frame until something else
        // happens, so collecting has to survive being asked repeatedly.
        let mut session = Session::new(7);
        force_win(&mut session);
        assert_eq!(session.granted(), Some(Item::Unlock(UNLOCKS[0])));
        session.retry();
        force_win(&mut session);
        assert_eq!(session.granted(), None, "the second clear had nothing left to give");
        assert!(session.inventory().has(Item::Unlock(UNLOCKS[0])), "and it kept the first");
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
