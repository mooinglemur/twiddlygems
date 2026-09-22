//! Plays every level with bots, to see whether the targets are worth aiming at.
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
use twiddlygems::progression::{solo_inventory, LONGEST_CHAIN, SHORTEST_CHAIN};

/// The ladder as a solo run actually meets it: every level narrowed to the
/// specials the levels below it have handed over.
///
/// Measuring it with everything switched on measures a game nobody plays. The
/// opener is meant to leave nothing behind at all, and the rungs just above it
/// are meant to be short of most of the pool, so those are the numbers that
/// say whether the ladder can be climbed.
fn ladder() -> Vec<LevelSpec> {
    let all = levels();
    let count = all.len();
    all.into_iter()
        .enumerate()
        .map(|(index, mut spec)| {
            solo_inventory(index, count).apply(index, &mut spec);
            spec
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Bot {
    First,
    Greedy,
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
    if !in_logic(25) {
        std::process::exit(1);
    }
}

/// How often each bot reaches a level's two score tiers.
///
/// A reading rather than a gate, but the one to set the numbers in the ladder
/// against. What they are aiming at: the attentive bot should reach gold most
/// of the time, or the tier is a location nobody can check, and the floor bot
/// should mostly miss it, or gold is what clearing the level already pays.
/// Silver sits where a good run lands rather than a lucky one.
///
/// Only wins count. A level that was not cleared has no tier, however high the
/// score got.
fn tiers(bot: Bot, seeds: u64) {
    let name = match bot {
        Bot::First => "first legal move",
        Bot::Greedy => "greedy",
    };
    println!("score tiers reached by the {name} bot ({seeds} seeds per level)");
    println!(
        "{:<16} {:>8} {:>9} {:>6} {:>9} {:>6}",
        "level", "won", "silver", "of won", "gold", "of won"
    );

    for (index, spec) in ladder().into_iter().enumerate() {
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
/// Counted per playthrough rather than per chain: what matters is whether a
/// run ever gets there, not how often.
fn chains(bot: Bot, seeds: u64) {
    println!("how deep a chain a playthrough reaches ({seeds} runs per level)");
    println!("{:<16}  {}", "length", "share of runs reaching it");

    let ladder = ladder();
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

/// Whether the ladder can be climbed at all with what it hands over.
///
/// This is the completability check, and it is the one number here that is a
/// gate rather than a reading. Every level has to be clearable holding only
/// what the levels below it gave, or a run dead-ends with nothing to do. The
/// greedy bot standing in for a player is generous, so a level it never wins
/// is one nobody can, whatever the rest of the table says about difficulty.
///
/// The same claim has to hold on the Archipelago side, where it will be asked
/// of received items rather than of the ladder. Both read the same inventory.
fn in_logic(seeds: u64) -> bool {
    println!("completable in logic: every level, with only what the ones below it gave");
    let mut ok = true;
    for (index, spec) in ladder().into_iter().enumerate() {
        let wins = (0..seeds)
            .filter(|seed| {
                play(&spec, seed * 7919 + index as u64, Bot::Greedy).status() == Status::Won
            })
            .count();
        let held = solo_inventory(index, levels().len()).specials();
        if wins == 0 {
            ok = false;
        }
        println!(
            "{:<16} {:>4}/{:<4} {} {:>3} moves, {}",
            spec.name,
            wins,
            seeds,
            if wins == 0 { "UNREACHABLE" } else { "ok         " },
            spec.moves,
            describe(held),
        );
    }
    if !ok {
        println!();
        println!("a level above cannot be cleared with what the ladder hands over before it");
    }
    ok
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
    println!("ceiling for the {name} bot ({seeds} seeds per level)");
    println!(
        "{:<16} {:>6} {:>9} {:>9}  {}",
        "level", "moves", "used p50", "used p90", "reached with the full budget"
    );

    for (index, spec) in ladder().into_iter().enumerate() {
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
fn specials_made(bot: Bot, seeds: u64) {
    println!("specials created per 100 moves ({seeds} seeds per level)");
    println!("{:<16} {:>8} {:>8} {:>8} {:>8} {:>8}", "level", "lineH", "lineV", "cross", "rainbow", "rocket");

    let mut overall = [0u32; 6];
    let mut total_moves = 0u32;
    let mut all_spreads: Vec<u32> = Vec::new();
    let mut all_voices: Vec<u32> = Vec::new();
    for (index, spec) in ladder().into_iter().enumerate() {
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

fn run(bot: Bot, seeds: u64) {
    let name = match bot {
        Bot::First => "first legal move",
        Bot::Greedy => "greedy, one move of lookahead",
    };
    println!("bot: {name} ({seeds} seeds per level)");
    println!(
        "{:<16} {:>6} {:>5} {:>8} {:>9}  {}",
        "level", "moves", "win%", "used", "score", "objectives (reached / needed)"
    );

    for (index, spec) in ladder().into_iter().enumerate() {
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
/// fraction of itself that is done, with score as a faint tie-break so the bot
/// prefers a bigger clear when nothing else separates two moves.
fn value(game: &Game) -> f64 {
    let mut total = 0.0;
    for objective in game.objectives() {
        let needed = objective.needed(&game.progress).max(1) as f64;
        total += objective.reached(&game.progress) as f64 / needed;
    }
    total + game.progress.score as f64 / 1.0e7
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
