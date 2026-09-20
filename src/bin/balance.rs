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

use twiddlygems::board::Pos;
use twiddlygems::game::{Game, Phase, Status};
use twiddlygems::level::{levels, LevelSpec, Objective};
use twiddlygems::matching;

#[derive(Clone, Copy, PartialEq)]
enum Bot {
    First,
    Greedy,
}

fn main() {
    // The greedy bot plays every candidate move out before choosing, so it gets
    // fewer runs; it is far more consistent, so it needs fewer.
    run(Bot::First, 200);
    println!();
    run(Bot::Greedy, 25);
    println!();
    calibrate(Bot::First, 200);
    println!();
    calibrate(Bot::Greedy, 25);
}

/// What the greedy bot can reach with the whole move budget, which is the
/// ceiling a target has to be set against.
///
/// Score and color goals are inflated out of reach so the level cannot end
/// early and the bot plays every move it has. Jelly cannot be inflated — it is
/// all or nothing — so for those levels the useful number is how many moves
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

    for (index, spec) in levels().into_iter().enumerate() {
        let mut used: Vec<u32> = Vec::new();
        for seed in 0..seeds {
            let game = play(&spec, seed * 7919 + index as u64, bot);
            if game.status() == Status::Won {
                used.push(spec.moves - game.moves_left);
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
            Objective::Jelly => Objective::Jelly,
        })
        .collect();
    probe
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

    for (index, spec) in levels().into_iter().enumerate() {
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
                used_when_won.push(moves - game.moves_left);
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
    }
}
