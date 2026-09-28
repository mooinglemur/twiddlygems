//! Plays every level with bots, to see whether the targets are worth aiming at.
//!
//! A measuring instrument, not a test. Nothing here runs under `make check`:
//! it is `make balance`, it takes half a minute, and what it produces is
//! tables to read. Its job is to answer questions that can only be answered by
//! playing a level a few hundred times, of which the live one is where to put
//! a level's silver and gold marks. Setting those by eye means guessing at a
//! score distribution; setting them off these tables means reading one.
//!
//! Two bots bracket the range of players:
//!
//! * `first` takes the first legal move it finds, top left to bottom right,
//!   with no thought for cascades, specials or where the jelly is. It is a
//!   floor: a level it clears comfortably is asking nothing of anyone.
//! * `greedy` tries every legal move, plays each one out, and keeps whichever
//!   made the most progress toward the level's objectives. It is closer to an
//!   attentive player, and it is the only fair read on positional goals like
//!   jelly, which the floor bot can only clear by accident.
//!
//!     cargo run --release --bin balance

use twiddlygems::board::{Pos, Special};
use twiddlygems::game::{Game, Phase, Status, EV_CLEAR, EV_SPECIAL_MADE};
use twiddlygems::level::{levels, LevelSpec, Objective};
use twiddlygems::matching;
use twiddlygems::progression::{
    Inventory, Item, LONGEST_CHAIN, LONGEST_MATCH, SHORTEST_CHAIN, SHORTEST_MATCH, UNLOCKS,
};

/// The ladder as the unluckiest run meets it: nothing in hand at all.
///
/// This is the floor, and it is the only state the rules promise. A run deals
/// its own progression now, so somebody will reach the top of the ladder
/// having found every unlock on score marks they never went back for. Anything
/// that has to be true of every run has to be true here.
fn bare() -> Vec<LevelSpec> {
    let all = levels();
    all.into_iter()
        .enumerate()
        .map(|(index, mut spec)| {
            Inventory::empty().apply(index, &mut spec);
            spec
        })
        .collect()
}

/// The ladder as a run that has found everything meets it: all five unlocks,
/// and every level's own move items.
///
/// This is the ceiling, and it is also exactly what a score mark's rule asks
/// for, so it is the state to read the tier numbers against. A level narrowed
/// to whatever one run happened to be holding would only ever measure that
/// run's luck.
///
/// Set up the way a fresh run is set up, since that is the game most people
/// will play. A run that turned its settings down is a different measurement
/// and would want its own table.
fn equipped() -> Vec<LevelSpec> {
    levels()
        .into_iter()
        .enumerate()
        .map(|(index, mut spec)| {
            let mut held = Inventory::empty();
            for special in UNLOCKS {
                held.receive(Item::Unlock(special));
            }
            held.receive(Item::Moves { level: index });
            held.apply(index, &mut spec);
            spec
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Bot {
    First,
    Greedy,
}

/// The move the floor bot takes: the first one the board allows, walking it
/// top left to bottom right.
///
/// Read off the board rather than asked of `Game::hint`, which is the player's
/// nudge and chooses at random from everything legal. This bot is the floor
/// every table here is read against, and a floor that plays a different game
/// each run is no floor at all.
fn first_move(game: &Game) -> Option<(Pos, Pos)> {
    matching::find_move(&game.board, game.rules())
}

fn main() {
    specials_made(Bot::First, 60);
    println!();
    // The greedy bot plays every candidate move out before choosing, so it gets
    // fewer runs; it is far more consistent, so it needs fewer.
    run(Bot::First, 200);
    println!();
    run(Bot::Greedy, 25);
    println!();
    calibrate(Bot::First, 200);
    println!();
    calibrate(Bot::Greedy, 25);
    println!();
    tiers(Bot::First, 200);
    println!();
    tiers(Bot::Greedy, 25);
    println!();
    chains(Bot::Greedy, 25);
    println!();
    match_sizes(Bot::Greedy, 25);
    println!();
    // More seeds than the other tables get, because the number that matters is
    // a tail rather than a middle: a bare run reaching a mark is rare by
    // design, and a handful of runs cannot tell rare from never.
    let marks_hold = marks(150);
    println!();
    gem_rate(50);
    println!();
    if !in_logic(25) || !marks_hold {
        std::process::exit(1);
    }
}

/// How long a player waits for an Archipelago gem to fall, at each frequency
/// the yaml offers.
///
/// A reading, not a gate. What it is for is picking the default: a gem should
/// be a pleasant surprise rather than something to grind for, and neither
/// "every other board" nor "never in a playthrough" is that.
///
/// Measured in playthroughs of one level rather than in gems per board,
/// because that is the unit a player feels: how many times they have to beat a
/// level before one turns up. A level is played to its end, win or lose, and
/// the gem is counted if it fell at all.
fn gem_rate(seeds: u64) {
    println!("how long an AP gem takes to turn up ({seeds} playthroughs per level)");
    println!(
        "{:<18}{:>10}{:>10}{:>10}{:>10}{:>12}",
        "one in", "L2 gems", "L2 runs", "L8 gems", "L8 runs", "runs per gem",
    );
    let ladder = bare();
    // The band the setting offers, at its ends, its default and a few places
    // in between, plus one on either side of it to show what was left out.
    // The doubling list this used to be measured a range the setting no
    // longer has: the useful values turned out to sit between 50 and 200.
    for odds in [32, 50, 75, 100, 150, 200, 256] {
        let mut row = Vec::new();
        for index in [1_usize, 7] {
            let mut gems = 0;
            let mut runs_with = 0;
            for seed in 0..seeds {
                let mut spec = ladder[index].clone();
                spec.rules.ap_gem_odds = odds;
                let mut game = Game::new(spec, seed * 7 + 1);
                // Plenty of room, so the count is about how often one falls
                // rather than about running out of checks.
                game.ap_gems_wanted = 99;
                let seen = play_counting_gems(&mut game);
                gems += seen;
                runs_with += u32::from(seen > 0);
            }
            row.push((gems, runs_with));
        }
        let total: u32 = row.iter().map(|(gems, _)| gems).sum();
        let per_gem =
            if total == 0 { f64::INFINITY } else { (seeds as f64 * 2.0) / total as f64 };
        println!(
            "{:<18}{:>10}{:>10}{:>10}{:>10}{:>12.1}",
            format!("1 in {odds}"),
            row[0].0,
            row[0].1,
            row[1].0,
            row[1].1,
            per_gem,
        );
    }
}

/// Plays one level out with the floor bot and counts the Archipelago gems that
/// fell, whether or not anything cleared them.
fn play_counting_gems(game: &mut Game) -> u32 {
    let mut seen = 0;
    let mut on_board = 0;
    for _ in 0..20_000 {
        if game.status() != Status::Playing {
            break;
        }
        if game.accepts_input() {
            match first_move(game) {
                Some((a, b)) => {
                    game.try_swap(a, b);
                }
                None => break,
            }
        }
        game.update(16.0);
        let now = game.board.ap_gems().len() as u32;
        if now > on_board {
            seen += now - on_board;
        }
        on_board = now;
    }
    seen
}

/// How often each bot reaches a level's two score tiers.
///
/// A reading rather than a gate, but the one to set the numbers in the ladder
/// against. What they are aiming at: the attentive bot should reach gold most
/// of the time, or the tier is a location nobody can check, and the floor bot
/// should mostly miss it, or gold is what clearing the level already pays.
/// Silver sits where a good run lands rather than a lucky one.
///
/// Read against [`equipped`], because that is what a mark's rule asks for: a
/// run is only expected at silver or gold once it holds the unlocks, and at
/// gold once it holds that level's moves as well.
///
/// Only wins count. A level that was not cleared has no tier, however high the
/// score got.
fn tiers(bot: Bot, seeds: u64) {
    let name = match bot {
        Bot::First => "first legal move",
        Bot::Greedy => "greedy",
    };
    println!(
        "score tiers reached by the {name} bot, holding everything ({seeds} seeds per level)"
    );
    println!(
        "{:<16} {:>8} {:>9} {:>6} {:>9} {:>6}",
        "level", "won", "silver", "of won", "gold", "of won"
    );

    for (index, spec) in equipped().into_iter().enumerate() {
        let mut won = 0;
        let (mut silver, mut gold) = (0, 0);
        for seed in 0..seeds {
            let game = play(&spec, seed * 7919 + index as u64, bot);
            if game.status() != Status::Won {
                continue;
            }
            won += 1;
            silver += u64::from(spec.silver > 0 && game.progress.score >= spec.silver);
            gold += u64::from(spec.gold > 0 && game.progress.score >= spec.gold);
        }
        let share = |n: u64| if won == 0 { "-".to_string() } else { format!("{}%", n * 100 / won) };
        println!(
            "{:<16} {:>8} {:>9} {:>6} {:>9} {:>6}",
            spec.name,
            won,
            spec.silver,
            share(silver),
            spec.gold,
            share(gold),
        );
    }
}

/// How deep a chain a playthrough actually reaches.
///
/// Each length is a location, so this says which of them anyone can check. A
/// length nothing ever reaches is a place items disappear into, and if logic
/// counted on one the run would dead-end.
///
/// Read against [`bare`], because a chain asks for nothing: its rule is
/// `Always`, so the run that has to be able to make one is the run holding
/// nothing. Measuring with the specials switched on would count chains that
/// only a run further along could ever make, and put items behind them.
///
/// Counted per playthrough rather than per chain: what matters is whether a
/// run ever gets there, not how often.
fn chains(bot: Bot, seeds: u64) {
    println!("how deep a chain a playthrough reaches, holding nothing ({seeds} runs per level)");
    println!("{:<16}  {}", "length", "share of runs reaching it");

    let ladder = bare();
    let mut reached = vec![0u64; (LONGEST_CHAIN + 2) as usize];
    let mut runs = 0u64;
    for (index, spec) in ladder.iter().enumerate() {
        for seed in 0..seeds {
            let mut game = Game::new(spec.clone(), seed * 7919 + index as u64);
            let mut deepest = 0;
            for _ in 0..4_000 {
                if game.status() != Status::Playing {
                    break;
                }
                if game.phase() == Phase::Idle {
                    let choice = match bot {
                        Bot::First => first_move(&game),
                        Bot::Greedy => best_move(&game),
                    };
                    match choice {
                        Some((a, b)) => {
                            game.try_swap(a, b);
                        }
                        None => break,
                    }
                }
                game.update(16.0);
                deepest = deepest.max(game.cascade());
            }
            runs += 1;
            for length in 0..=deepest.min(LONGEST_CHAIN) {
                reached[length as usize] += 1;
            }
        }
    }

    for length in SHORTEST_CHAIN..=LONGEST_CHAIN {
        let hits = reached[length as usize];
        println!(
            "{:<16}  {:>4}% {}",
            format!("{length} Chain"),
            hits * 100 / runs.max(1),
            if hits == 0 { "  NOBODY EVER GETS HERE" } else { "" },
        );
    }
}

/// How often a playthrough lines up exactly each match size.
///
/// The table [`RELIABLE_MATCH`] is set off, and the same question the chains
/// table asks: a location a run rarely reaches is one a solo player would be
/// asked to be lucky to finish their own progression at.
///
/// Exactly, because that is what these locations ask for: a six does not pay
/// the five, so growing into bigger matches does not fill the smaller ones in
/// on the way. Read against a run holding nothing, which is what their rule
/// asks for, and counted per playthrough rather than per move: what a player
/// feels is how many times they have to play a level before one happens.
fn match_sizes(bot: Bot, seeds: u64) {
    println!("how often a move lines up exactly this many gems, holding nothing \
              ({seeds} runs per level)");
    // The opening level on its own as well, because these ask for no item at
    // all: they are sphere one, so a solo run may keep its first progression
    // behind one while only the opener is unlocked.
    println!("{:<20}  {:>12}  {:>12}", "size", "whole ladder", "opener only");

    let ladder = bare();
    let mut reached = vec![0u64; (LONGEST_MATCH + 2) as usize];
    let mut opener = vec![0u64; (LONGEST_MATCH + 2) as usize];
    let mut runs = 0u64;
    let mut opener_runs = 0u64;
    for (index, spec) in ladder.iter().enumerate() {
        for seed in 0..seeds {
            let mut game = Game::new(spec.clone(), seed * 7919 + index as u64);
            let mut seen = vec![false; (LONGEST_MATCH + 2) as usize];
            for _ in 0..4_000 {
                if game.status() != Status::Playing {
                    break;
                }
                if game.phase() == Phase::Idle {
                    let choice = match bot {
                        Bot::First => first_move(&game),
                        Bot::Greedy => best_move(&game),
                    };
                    match choice {
                        Some((a, b)) => {
                            game.try_swap(a, b);
                        }
                        None => break,
                    }
                }
                game.update(16.0);
                let lined_up = game.swap_match();
                if (SHORTEST_MATCH..=LONGEST_MATCH).contains(&lined_up) {
                    seen[lined_up as usize] = true;
                }
            }
            runs += 1;
            opener_runs += u64::from(index == 0);
            for size in SHORTEST_MATCH..=LONGEST_MATCH {
                reached[size as usize] += u64::from(seen[size as usize]);
                if index == 0 {
                    opener[size as usize] += u64::from(seen[size as usize]);
                }
            }
        }
    }

    for size in SHORTEST_MATCH..=LONGEST_MATCH {
        let hits = reached[size as usize];
        println!(
            "{:<20}  {:>11}%  {:>11}% {}",
            format!("Activate {size} match"),
            hits * 100 / runs.max(1),
            opener[size as usize] * 100 / opener_runs.max(1),
            if hits == 0 { "  NOBODY EVER GETS HERE" } else { "" },
        );
    }
}

/// What a score mark has to clear, which is the table its numbers are set off.
///
/// Both ends of the same level, side by side. A mark is meant to be something
/// a well supplied run chases, and the rules say as much: silver and gold both
/// ask for all five unlocks, because nearly all of a good score comes from the
/// flourish and a run holding nothing has nothing to mint. So a mark below
/// what a bare run scores anyway is a location that pays for nothing, and it
/// makes the rule a lie as well: logic promises the unlocks are needed.
///
/// Read it by putting silver above the bare column and gold well above it,
/// then checking both sit inside the full column often enough to be worth
/// chasing. Only wins count, since a level that was not cleared has no tier.
///
/// **The last column is a gate on the opening level**, and a reading for the
/// rest. First Light is the one level designed rather than bracketed so far,
/// and its marks are meant to be out of reach without specials: a bare run
/// reaching one there is the failure this exists to catch. The rest of the
/// ladder is placeholder and reaches its own marks bare all day, which is
/// what the column says. Widen the gate as levels are redesigned.
fn marks(seeds: u64) -> bool {
    println!("what a score mark has to clear ({seeds} seeds per level)");
    println!(
        "{:<16} {:>9} {:>9} {:>9} {:>10} {:>9} {:>9}  {}",
        "level", "bare p50", "bare max", "full p50", "full p90", "silver", "gold",
        "bare reaches them",
    );

    let mut opener_holds = true;
    let bare = bare();
    for (index, spec) in equipped().into_iter().enumerate() {
        let mut nothing: Vec<u64> = Vec::new();
        let mut everything: Vec<u64> = Vec::new();
        for seed in 0..seeds {
            let seed = seed * 7919 + index as u64;
            let game = play(&bare[index], seed, Bot::Greedy);
            if game.status() == Status::Won {
                nothing.push(game.progress.score);
            }
            let game = play(&spec, seed, Bot::Greedy);
            if game.status() == Status::Won {
                everything.push(game.progress.score);
            }
        }
        // Counted rather than shared out. A percentage of a few hundred runs
        // rounds one hit down to nothing, and one bare run reaching a mark is
        // the whole of what this is looking for.
        let hits = |mark: u64| {
            if mark == 0 {
                return 0;
            }
            nothing.iter().filter(|score| **score >= mark).count()
        };
        let (bare_silver, bare_gold) = (hits(spec.silver), hits(spec.gold));
        // One run in a hundred, not none. Three moves on a narrow board has a
        // long tail: a bare run occasionally cascades into something enormous,
        // and a mark set above that would be one no supplied run could reach
        // either. What this is looking for is a mark a bare run reaches
        // routinely, which is a mark that pays for nothing.
        let allowance = nothing.len() / 100;
        if index == 0 && (bare_silver > allowance || bare_gold > allowance) {
            opener_holds = false;
        }
        let at = |values: &mut Vec<u64>, p: usize| {
            percentile(values, p).map_or("-".to_string(), |score| score.to_string())
        };
        println!(
            "{:<16} {:>9} {:>9} {:>9} {:>10} {:>9} {:>9}  {}",
            spec.name,
            at(&mut nothing, 50),
            at(&mut nothing, 100),
            at(&mut everything, 50),
            at(&mut everything, 90),
            spec.silver,
            spec.gold,
            format!(
                "{bare_silver} / {bare_gold} of {}{}",
                nothing.len(),
                if index == 0 && (bare_silver > allowance || bare_gold > allowance) {
                    "   TOO EASY"
                } else {
                    ""
                },
            ),
        );
    }
    if !opener_holds {
        println!();
        println!("the opening level's marks can be reached without a single special, which is");
        println!("not what they are for: every mark's rule asks for all five unlocks");
    }
    opener_holds
}

/// How far up the ladder a run that has found nothing can get.
///
/// Mostly a reading. A level that wants its move items before it will go down
/// is a fine level, and with several move items per level it is the expected
/// shape; what it is not is free. A run deals its own progression, so whether
/// those moves are in hand when the level comes up is the seed's business, and
/// the only thing that makes it safe is the level's own rule saying so.
/// Nothing here can tell a level that is meant to want items from one that is
/// short by accident, so it reports and leaves the judgment to whoever is
/// designing the ladder.
///
/// **The opener is the exception, and it is a gate.** Its rule is `Always`: a
/// run holding nothing has to be able to clear it, because until it does, not
/// one location in the game is open and there is no first item to find. A seed
/// where that fails is a seed nobody can start. The greedy bot standing in for
/// a player is generous, so a level it never wins is one nobody can.
fn in_logic(seeds: u64) -> bool {
    println!("how far a run that has found nothing gets, level by level");
    let mut opener = true;
    for (index, spec) in bare().into_iter().enumerate() {
        let wins = (0..seeds)
            .filter(|seed| {
                play(&spec, seed * 7919 + index as u64, Bot::Greedy).status() == Status::Won
            })
            .count();
        if wins == 0 && index == 0 {
            opener = false;
        }
        println!(
            "{:<16} {:>4}/{:<4} {} {:>3} moves, {}",
            spec.name,
            wins,
            seeds,
            match (index, wins) {
                (0, 0) => "THE OPENER MUST BE CLEARABLE WITH NOTHING",
                (_, 0) => "wants items before it will go down     ",
                _ => "                                       ",
            },
            spec.moves,
            describe(spec.rules.specials),
        );
    }
    if !opener {
        println!();
        println!("the opening level cannot be cleared by a run holding nothing, so a run");
        println!("has nowhere to find its first item and no seed can be started");
    }
    opener
}

/// The specials a run holds, for the logic table.
fn describe(held: twiddlygems::rules::SpecialSet) -> String {
    let names = [
        (held.line_h, "lineH"),
        (held.line_v, "lineV"),
        (held.cross, "cross"),
        (held.rainbow, "rainbow"),
        (held.rocket, "rocket"),
    ];
    let holding: Vec<&str> = names.iter().filter(|(on, _)| *on).map(|(_, name)| *name).collect();
    if holding.is_empty() {
        "holding nothing".to_string()
    } else {
        format!("holding {}", holding.join(" "))
    }
}

/// What the greedy bot can reach with the whole move budget, which is the
/// ceiling a target has to be set against.
///
/// Score and color goals are inflated out of reach so the level cannot end
/// early and the bot plays every move it has. Jelly cannot be inflated (it is
/// all or nothing), so for those levels the useful number is how many moves
/// clearing the board actually took.
fn calibrate(bot: Bot, seeds: u64) {
    let name = match bot {
        Bot::First => "first legal move",
        Bot::Greedy => "greedy",
    };
    println!("ceiling for the {name} bot, holding everything ({seeds} seeds per level)");
    println!(
        "{:<16} {:>6} {:>9} {:>9}  {}",
        "level", "moves", "used p50", "used p90", "reached with the full budget"
    );

    for (index, spec) in equipped().into_iter().enumerate() {
        let mut used: Vec<u32> = Vec::new();
        for seed in 0..seeds {
            let game = play(&spec, seed * 7919 + index as u64, bot);
            if game.status() == Status::Won {
                used.push(spec.moves.saturating_sub(game.progress.moves_spare));
            }
        }

        let probe = inflate(&spec);
        let mut ceiling: Vec<Vec<u32>> = vec![Vec::new(); probe.objectives.len()];
        for seed in 0..seeds {
            let game = play(&probe, seed * 7919 + index as u64, bot);
            for (i, objective) in probe.objectives.iter().enumerate() {
                ceiling[i].push(objective.reached(&game.progress));
            }
        }

        let detail: Vec<String> = probe
            .objectives
            .iter()
            .enumerate()
            .map(|(i, objective)| {
                format!("{} {}", label(objective), median(&mut ceiling[i]).unwrap_or(0))
            })
            .collect();

        println!(
            "{:<16} {:>6} {:>9} {:>9}  {}",
            spec.name,
            spec.moves,
            median(&mut used.clone()).map_or("-".to_string(), |m| m.to_string()),
            percentile(&mut used, 90).map_or("-".to_string(), |m| m.to_string()),
            detail.join(", "),
        );
    }
}

/// The same level with its thresholds pushed out of reach, so a run uses every
/// move. The objective kinds are kept so the bot still aims at the right thing.
fn inflate(spec: &LevelSpec) -> LevelSpec {
    let mut probe = spec.clone();
    probe.objectives = spec
        .objectives
        .iter()
        .map(|objective| match objective {
            Objective::Score(target) => Objective::Score(target.saturating_mul(5)),
            Objective::Color { color, count } => {
                Objective::Color { color: *color, count: count.saturating_mul(5) }
            }
            // Jelly and brick are all or nothing, so there is nothing to
            // inflate: the useful number for those is how many moves clearing
            // the board actually took.
            Objective::Jelly => Objective::Jelly,
            Objective::Brick => Objective::Brick,
            Objective::Seal { color } => Objective::Seal { color: *color },
        })
        .collect();
    probe
}

/// How often each special actually turns up in play.
///
/// Read against [`equipped`]: a run that has not found an unlock never makes
/// that special at all, so the rates only mean anything once they are all on.
fn specials_made(bot: Bot, seeds: u64) {
    println!("specials created per 100 moves, holding everything ({seeds} seeds per level)");
    println!("{:<16} {:>8} {:>8} {:>8} {:>8} {:>8}", "level", "lineH", "lineV", "cross", "rainbow", "rocket");

    let mut overall = [0u32; 6];
    let mut total_moves = 0u32;
    let mut all_spreads: Vec<u32> = Vec::new();
    let mut all_voices: Vec<u32> = Vec::new();
    for (index, spec) in equipped().into_iter().enumerate() {
        let mut made = [0u32; 6];
        let mut moves = 0u32;
        let mut spreads: Vec<u32> = Vec::new();
        let mut voices: Vec<u32> = Vec::new();
        for seed in 0..seeds {
            let game = play_counting(
                &spec,
                seed * 7919 + index as u64,
                bot,
                &mut made,
                &mut spreads,
                &mut voices,
            );
            // Moves the player actually made, which is what the counts below
            // are per hundred of. The flourish at the end of a level spends
            // the rest without anyone swapping anything.
            moves += spec.moves.saturating_sub(game.progress.moves_spare.max(game.moves_left));
        }
        all_spreads.extend_from_slice(&spreads);
        all_voices.extend_from_slice(&voices);
        let per100 = |n: u32| if moves == 0 { 0.0 } else { n as f64 * 100.0 / moves as f64 };
        println!(
            "{:<16} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1}",
            spec.name,
            per100(made[Special::LineH.code() as usize]),
            per100(made[Special::LineV.code() as usize]),
            per100(made[Special::Cross.code() as usize]),
            per100(made[Special::Rainbow.code() as usize]),
            per100(made[Special::Rocket.code() as usize]),
        );
        for (slot, value) in made.iter().enumerate() {
            overall[slot] += value;
        }
        total_moves += moves;
    }
    println!();
    println!("how long a clear takes to finish rippling, in milliseconds:");
    let spread_at = |p: usize| percentile(&mut all_spreads.clone(), p).unwrap_or(0);
    let instant = all_spreads.iter().filter(|s| **s == 0).count();
    println!(
        "  p50 {}   p90 {}   p99 {}   worst {}   ({:.0}% of clears are instant)",
        spread_at(50),
        spread_at(90),
        spread_at(99),
        spread_at(100),
        instant as f64 * 100.0 / all_spreads.len().max(1) as f64,
    );
    println!();
    println!(
        "gems clearing within the same {SOUND_WINDOW_MS}ms, which is how many sounds would fire at once:"
    );
    let voices_at = |p: usize| percentile(&mut all_voices.clone(), p).unwrap_or(0);
    println!(
        "  p50 {}   p90 {}   p99 {}   worst {}",
        voices_at(50),
        voices_at(90),
        voices_at(99),
        voices_at(100),
    );
    println!();

    let per100 = |n: u32| n as f64 * 100.0 / total_moves.max(1) as f64;
    println!(
        "{:<16} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1}",
        "ALL",
        per100(overall[1]),
        per100(overall[2]),
        per100(overall[3]),
        per100(overall[4]),
        per100(overall[5]),
    );
}

/// How each bot does on the ladder, read against [`bare`].
///
/// The win column is the one the levels are designed against, and a level has
/// to be winnable by a run that found nothing, so this is the state to design
/// against. What a well supplied run does with the same level is the tier
/// table's business.
fn run(bot: Bot, seeds: u64) {
    let name = match bot {
        Bot::First => "first legal move",
        Bot::Greedy => "greedy, one move of lookahead",
    };
    println!("bot: {name}, holding nothing ({seeds} seeds per level)");
    println!(
        "{:<16} {:>6} {:>5} {:>8} {:>9}  {}",
        "level", "moves", "win%", "used", "score", "objectives (reached / needed)"
    );

    for (index, spec) in bare().into_iter().enumerate() {
        let moves = spec.moves;
        let mut wins = 0;
        let mut used_when_won: Vec<u32> = Vec::new();
        let mut scores: Vec<u64> = Vec::new();
        let mut reached: Vec<Vec<u32>> = vec![Vec::new(); spec.objectives.len()];
        let mut needed: Vec<u32> = vec![0; spec.objectives.len()];

        for seed in 0..seeds {
            let game = play(&spec, seed * 7919 + index as u64, bot);
            scores.push(game.progress.score);
            for (i, objective) in spec.objectives.iter().enumerate() {
                reached[i].push(objective.reached(&game.progress));
                needed[i] = objective.needed(&game.progress);
            }
            if game.status() == Status::Won {
                wins += 1;
                // From `moves_spare` rather than the counter: the end of a
                // level spends whatever was left, so afterwards the counter
                // always reads zero and every level would look like it needed
                // its whole budget.
                used_when_won.push(moves.saturating_sub(game.progress.moves_spare));
            }
        }

        let detail: Vec<String> = spec
            .objectives
            .iter()
            .enumerate()
            .map(|(i, objective)| {
                format!(
                    "{} {}/{}",
                    label(objective),
                    median(&mut reached[i]).unwrap_or(0),
                    needed[i]
                )
            })
            .collect();

        println!(
            "{:<16} {:>6} {:>4}% {:>8} {:>9}  {}",
            spec.name,
            moves,
            wins * 100 / seeds,
            median(&mut used_when_won).map_or("-".to_string(), |m| m.to_string()),
            median(&mut scores).unwrap_or(0),
            detail.join(", "),
        );
    }
}

/// Plays a level, tallying which specials the run actually produced. Counting
/// them is the only way to tell a rule that is rare from one that is dead.
fn play_counting(
    spec: &LevelSpec,
    seed: u64,
    bot: Bot,
    made: &mut [u32; 6],
    spreads: &mut Vec<u32>,
    voices: &mut Vec<u32>,
) -> Game {
    let mut game = Game::new(spec.clone(), seed);
    for _ in 0..4_000 {
        if game.status() != Status::Playing {
            break;
        }
        if game.phase() == Phase::Idle {
            let choice = match bot {
                Bot::First => game.hint(),
                Bot::Greedy => best_move(&game),
            };
            match choice {
                Some((a, b)) => {
                    game.try_swap(a, b);
                }
                None => break,
            }
        }
        for _ in 0..64 {
            if game.phase() == Phase::Idle || game.phase() == Phase::Finished {
                break;
            }
            game.update(250.0);
            // Every clear arrives as one batch of events, so the widest delay
            // in a batch is how long that clear takes to finish rippling, and
            // the fullest window within it is how many sounds land at once.
            let mut widest = None;
            let mut delays: Vec<u32> = Vec::new();
            for event in game.events() {
                if event.kind == EV_SPECIAL_MADE && (event.special as usize) < made.len() {
                    made[event.special as usize] += 1;
                }
                if event.kind == EV_CLEAR {
                    widest = Some(widest.unwrap_or(0).max(event.value as u32));
                    delays.push(event.value as u32);
                }
            }
            if let Some(widest) = widest {
                spreads.push(widest);
                voices.push(busiest_window(&mut delays, SOUND_WINDOW_MS));
            }
        }
    }
    game
}

fn play(spec: &LevelSpec, seed: u64, bot: Bot) -> Game {
    let mut game = Game::new(spec.clone(), seed);
    for _ in 0..4_000 {
        if game.status() != Status::Playing {
            break;
        }
        if game.phase() == Phase::Idle {
            let choice = match bot {
                Bot::First => game.hint(),
                Bot::Greedy => best_move(&game),
            };
            match choice {
                Some((a, b)) => {
                    game.try_swap(a, b);
                }
                None => break,
            }
        }
        settle(&mut game);
    }
    game
}

/// Runs the board to rest. Large steps rather than frames: the state machine
/// carries leftover time across phases, so this lands in the same place a
/// browser would, in a fraction of the calls.
fn settle(game: &mut Game) {
    for _ in 0..64 {
        if game.phase() == Phase::Idle || game.phase() == Phase::Finished {
            return;
        }
        game.update(250.0);
    }
}

/// Every legal move, tried and scored by what it actually achieved.
fn best_move(game: &Game) -> Option<(Pos, Pos)> {
    let before = value(game);
    let mut best: Option<(Pos, Pos)> = None;
    let mut best_gain = f64::MIN;

    for p in game.board.positions() {
        for q in [Pos::new(p.r, p.c + 1), Pos::new(p.r + 1, p.c)] {
            if !game.board.contains(q) || !matching::is_useful_swap(&game.board, game.rules(), p, q)
            {
                continue;
            }
            let mut trial = game.clone();
            if !trial.try_swap(p, q) {
                continue;
            }
            settle(&mut trial);
            let gain = value(&trial) - before;
            if gain > best_gain {
                best_gain = gain;
                best = Some((p, q));
            }
        }
    }
    best
}

/// How close this board is to clearing the level: each objective counts for the
/// fraction of itself that is done, then damage that has not finished anything
/// yet, then score, each a long way below the one before it.
///
/// The counter a player reads moves only when a cell is finished, which is
/// right for a player and useless as a gradient: softening a double jelly or
/// cracking a brick would score nothing at all, and the bot could not tell the
/// move that did it from one that did nothing. So the hits are counted here as
/// well, far enough below a finished cell that they only ever break a tie. The
/// bot lost a third of its wins on Landslide before they were.
fn value(game: &Game) -> f64 {
    let mut total = 0.0;
    for objective in game.objectives() {
        let needed = objective.needed(&game.progress).max(1) as f64;
        total += objective.reached(&game.progress) as f64 / needed;
    }
    let left: u32 = game
        .board
        .positions()
        .map(|p| game.board.jelly(p) as u32 + game.board.brick(p) as u32)
        .sum();
    total - left as f64 * 1.0e-3 + game.progress.score as f64 / 1.0e7
}

/// Roughly one frame: sounds starting this close together are heard as one
/// moment, and are what a voice budget has to cover.
const SOUND_WINDOW_MS: u32 = 30;

/// The most cells that pop within any `window` of each other.
fn busiest_window(delays: &mut Vec<u32>, window: u32) -> u32 {
    delays.sort_unstable();
    let mut best = 0;
    let mut start = 0;
    for end in 0..delays.len() {
        while delays[end] - delays[start] > window {
            start += 1;
        }
        best = best.max((end - start + 1) as u32);
    }
    best
}

fn percentile<T: Copy + Ord>(values: &mut Vec<T>, p: usize) -> Option<T> {
    if values.is_empty() {
        return None;
    }
    values.sort();
    let index = (values.len() - 1) * p / 100;
    Some(values[index])
}

fn median<T: Copy + Ord>(values: &mut Vec<T>) -> Option<T> {
    percentile(values, 50)
}

fn label(objective: &Objective) -> &'static str {
    match objective {
        Objective::Score(_) => "score",
        Objective::Color { .. } => "color",
        Objective::Jelly => "jelly",
        Objective::Brick => "brick",
        Objective::Seal { .. } => "seal",
    }
}
