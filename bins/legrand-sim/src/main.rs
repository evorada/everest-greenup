//! `legrand-sim --board 0.0.0.0:5101 --control 0.0.0.0:5102 [--profile NAME] [--side N] [--script "2:plug,5:draw,60:unplug"]`

use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

use board_model::Profile;
use legrand_sim::{Script, Simulator};

fn usage() -> ExitCode {
    eprintln!(
        "usage: legrand-sim [--board ADDR] [--control ADDR] [--profile NAME] [--side N] [--script \"SECS:CMD,...\"]\n\
         profiles: {}\n\
         script commands: plug, unplug, draw, stop, full",
        Profile::all()
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut board: SocketAddr = match "0.0.0.0:5101".parse() {
        Ok(a) => a,
        Err(_) => return usage(),
    };
    let mut control: SocketAddr = match "0.0.0.0:5102".parse() {
        Ok(a) => a,
        Err(_) => return usage(),
    };
    let mut profile = Profile::default();
    let mut script: Script = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let Some(value) = args.next() else {
            return usage();
        };
        match flag.as_str() {
            "--board" => match value.parse() {
                Ok(a) => board = a,
                Err(_) => return usage(),
            },
            "--control" => match value.parse() {
                Ok(a) => control = a,
                Err(_) => return usage(),
            },
            "--profile" => match Profile::all().into_iter().find(|(n, _)| *n == value) {
                Some((_, p)) => {
                    profile = Profile {
                        side: profile.side,
                        ..p
                    }
                }
                None => return usage(),
            },
            "--side" => match value.parse() {
                Ok(side) => profile.side = side,
                Err(_) => return usage(),
            },
            "--script" => {
                for item in value.split(',').filter(|s| !s.is_empty()) {
                    let Some((at, command)) = item.split_once(':') else {
                        return usage();
                    };
                    let Ok(at) = at.trim().parse::<f64>() else {
                        return usage();
                    };
                    script.push((at, command.trim().to_owned()));
                }
            }
            _ => return usage(),
        }
    }
    let sim = match Simulator::start(board, control, profile, script) {
        Ok(sim) => sim,
        Err(e) => {
            eprintln!("legrand-sim: cannot listen: {e}");
            return ExitCode::from(1);
        }
    };
    println!(
        "legrand-sim: board on {}, control on {}",
        sim.board_addr(),
        sim.control_addr()
    );
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}
