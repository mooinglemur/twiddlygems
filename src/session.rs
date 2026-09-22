//! A run through the level ladder.
//!
//! [`Session`] is the seam where progression lives: today it unlocks the next
//! level when you clear one, and later the Archipelago layer will answer the
//! same question (which levels may be played) from received items instead.

use crate::game::{Game, Status};
use crate::level::{levels, LevelSpec};
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
    game: Game,
    name_buf: Vec<u8>,
    names_blob: Vec<u8>,
}

impl Session {
    pub fn new(seed: u64) -> Self {
        let levels = levels();
        let first = levels.first().cloned().expect("the ladder is never empty");
        let mut deal = Rng::new(seed);
        let mut session = Session {
            game: Game::new(first, deal.next_u64()),
            levels,
            index: 0,
            unlocked: 1,
            deal,
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

    /// Opens the current level on the next board the run has to give.
    fn deal_level(&mut self) {
        let seed = self.deal.next_u64();
        self.game = Game::new(self.levels[self.index].clone(), seed);
        self.sync_name();
    }

    pub fn update(&mut self, dt_ms: f32) {
        self.game.update(dt_ms);
        if self.game.status() == Status::Won {
            self.unlocked = self.unlocked.max((self.index + 2).min(self.levels.len()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Phase;

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
        // Force a win rather than playing one out: the unlock is what is
        // under test, not the level's difficulty.
        session.game_mut().progress.score = 1_000_000;
        while session.game().phase() != Phase::Finished {
            let hint = session.game().hint();
            if let Some((a, b)) = hint {
                session.game_mut().try_swap(a, b);
            }
            session.update(16.0);
        }
        assert_eq!(session.game().status(), Status::Won);
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
