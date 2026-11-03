use std::sync::OnceLock;

pub fn spawn_interval() -> u32 {
    static N: OnceLock<u32> = OnceLock::new();
    *N.get_or_init(|| std::env::var("BELUMI_SPAWN_INTERVAL").ok().and_then(|v| v.parse().ok()).filter(|n| *n >= 1).unwrap_or(1))
}

pub fn spawn_attempts_this_tick(game_time: i64, interval: u32) -> bool {
    interval <= 1 || game_time % i64::from(interval) == 0 || game_time % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::spawn_attempts_this_tick as run;

    #[test]
    fn interval_one_is_vanilla_and_animal_pass_always_runs() {
        assert!((0..1000).all(|t| run(t, 1)));
        let ran: Vec<i64> = (0..9).filter(|t| run(*t, 4)).collect();
        assert_eq!(ran, vec![0, 4, 8]);
        assert!(run(400, 3), "400 is not a multiple of 3 but the animal pass still runs");
        assert!(!run(401, 3));
    }
}
