//! A run through the level ladder.
//!
//! [`Session`] is the seam where progression lives: today it unlocks the next
//! level when you clear one, and later the Archipelago layer will answer the
//! same question (which levels may be played) from received items instead.

use crate::game::{Event, Game, Status, EV_ITEM};
use crate::level::{levels, LevelSpec};
use crate::progression::{
    solo_item_at, Inventory, Item, Location, Tier, LONGEST_CHAIN, NO_LOCATION, SHORTEST_CHAIN,
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
            game: open_level(0, first, &inventory, deal.next_u64()),
            levels: levels.clone(),
            index: 0,
            unlocked: 1,
            deal,
            inventory,
            checked: Vec::new(),
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
            let place = from.map_or((NO_LOCATION, 0), |at| (at.kind(), at.param()));
            self.events.push(Event::about_item(
                EV_ITEM,
                (item.kind(), item.value() as u8),
                place,
            ));
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
        if let Some(item) = solo_item_at(location) {
            self.receive_from(item, Some(location));
        }
    }

    /// Which locations this run has checked, for writing down.
    pub fn checked(&self) -> &[u32] {
        &self.checked
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
        if let Some(item) = solo_item_at(location) {
            self.inventory.receive(item);
        }
        self.refresh_specials();
        self.refresh_moves();
    }

    /// Checks whatever the board has earned this frame.
    fn check_reached(&mut self) {
        if self.game.status() == Status::Won {
            self.check(Location::LevelClear(self.index));
            // Only once the level is over, because the flourish is still
            // adding to the score right up until then.
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
    use crate::progression::UNLOCKS;
    use crate::rules::SpecialSet;

    /// Wins the level in front of the session without playing it properly.
    ///
    /// The score is put out of reach and then the board is nudged until it
    /// ends, because what these tests are about is what a clear leads to, not
    /// whether the level is beatable.
    /// What an item event says, unpacked: the item, then where it came from.
    type Announced = ((u8, u8), (u8, u16));

    fn announced(events: &[Event]) -> Vec<Announced> {
        events
            .iter()
            .filter(|e| e.kind == EV_ITEM)
            .map(|e| ((e.color, e.special), (e.cascade, e.value)))
            .collect()
    }

    fn says(item: Item, from: Option<Location>) -> Announced {
        (
            (item.kind(), item.value() as u8),
            from.map_or((NO_LOCATION, 0), |at| (at.kind(), at.param())),
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
    fn clearing_a_level_hands_over_an_unlock_the_next_one_can_use() {
        let mut session = Session::new(7);
        let seen = force_win(&mut session);
        assert!(
            seen.contains(&says(
                Item::Unlock(UNLOCKS[0]),
                Some(Location::LevelClear(0)),
            )),
            "clearing the opener should hand over the first of the pool, and say where from",
        );

        assert!(session.next_level());
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
        let first = force_win(&mut session);
        assert!(first.iter().any(|said| said.0 .0 == Item::Unlock(UNLOCKS[0]).kind()));
        session.retry();
        let again = force_win(&mut session);
        // Other locations may well pay on the way (the end-of-level flourish
        // can chain, and chains are locations too). What must not happen is
        // this level's own clear paying a second time.
        let clear = Location::LevelClear(0);
        assert!(
            !again.iter().any(|(_, from)| *from == (clear.kind(), clear.param())),
            "clearing the same level paid its location twice",
        );
        assert!(session.inventory().has(Item::Unlock(UNLOCKS[0])), "and it kept the first");
    }

    #[test]
    fn a_score_past_a_tier_checks_it_and_a_score_short_of_one_does_not() {
        let marks = |session: &Session| (session.level().silver, session.level().gold);

        // Short of silver: the level is cleared and neither tier is.
        let mut session = Session::new(7);
        let (silver, gold) = marks(&session);
        assert!(silver > 0 && gold > silver, "the opener has no tiers to test");
        force_win_at(&mut session, silver - 1);
        assert!(session.checked().contains(&Location::LevelClear(0).id()));
        assert!(
            !session.checked().contains(&Location::LevelSilver(0).id()),
            "a score short of silver checked it anyway",
        );

        // Past silver but short of gold.
        let mut session = Session::new(7);
        force_win_at(&mut session, gold - 1);
        assert!(session.checked().contains(&Location::LevelSilver(0).id()), "silver was missed");
        assert!(
            !session.checked().contains(&Location::LevelGold(0).id()),
            "a score short of gold checked it anyway",
        );

        // Past both. Exactly on the mark counts as reaching it.
        let mut session = Session::new(7);
        force_win_at(&mut session, gold);
        assert!(session.checked().contains(&Location::LevelSilver(0).id()));
        assert!(session.checked().contains(&Location::LevelGold(0).id()), "gold was missed");
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
        assert_eq!(
            session.inventory().moves_found(0),
            1,
            "the chain should have paid what the table says",
        );

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
        let mut session = Session::new(7);
        session.restore(Location::LevelClear(0).id());
        session.restore(Location::Chain(SHORTEST_CHAIN).id());

        assert!(session.inventory().has(Item::Unlock(UNLOCKS[0])), "the unlock did not come back");
        assert_eq!(session.inventory().moves_found(0), 1, "the moves did not come back");
        assert!(session.events().is_empty(), "restoring announced itself");
        assert_eq!(
            session.game().rules().specials,
            SpecialSet { line_v: true, ..SpecialSet::NONE },
            "and the board in play should be what the restored run may do",
        );
    }

    #[test]
    fn a_restored_location_cannot_be_found_again() {
        let mut session = Session::new(7);
        let chain = Location::Chain(SHORTEST_CHAIN).id();
        session.restore(chain);
        session.restore(chain);
        assert_eq!(session.inventory().moves_found(0), 1, "restoring twice paid twice");
        assert_eq!(session.checked(), [chain], "and it was written down twice");
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
