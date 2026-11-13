use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::exit;
use tracing::error;

use crate::crypto;
pub fn hash_to_line(input: &str) -> String {
    // Hash the input
    let hash = Sha256::digest(input.as_bytes());
    let hash_num = u64::from_be_bytes(hash[0..8].try_into().unwrap());
    let mut cwd = std::env::current_dir().unwrap();
    cwd.push(PathBuf::from("src/nameslist/names.csv"));

    if !cwd.exists() {
        cwd = PathBuf::from("/app/names.csv");
        if !cwd.exists() {
            error!(
                "Could not find names.csv file ! Closing everything. Wrong path : {:?}",
                cwd
            );
            exit(1);
        }
    }
    tracing::debug!("Using this name file : {:?}", cwd);
    // Read all lines
    let lines: Vec<String> = fs::read_to_string(cwd)
        .unwrap_or_else(|err| {
            error!("Could not find names.csv file ! Closing everything. {err}");
            exit(1)
        })
        .lines()
        .map(String::from)
        .collect();

    // Select line using modulo
    let index = (hash_num % lines.len() as u64) as usize;
    let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let choosen_letter = (hash_num % alphabet.len() as u64) as usize;
    let letter = &alphabet[choosen_letter..choosen_letter + 1];
    format!(
        "{} {} {}",
        lines[index].clone(),
        letter,
        input[4..8].to_string()
    )
}
pub fn testnames() {
    for i in 0..10000000000_i64 {
        let istr = i.to_string();
        let room_name = hash_to_line(&crypto::hash_phone(&String::from(istr), "putyourhashhere"));
        tracing::debug!("room_name={room_name}");
        // Use &istr or istr.as_str() for &str
    }
}
