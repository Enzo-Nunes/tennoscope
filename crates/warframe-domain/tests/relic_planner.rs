use warframe_domain::{Currency, PlannedRelic, PlannedReward, Refinement, expected_best, plan};

/// The published chances, as fractions: three commons, two uncommons, one rare.
const COMMON: [f64; 4] = [0.2533, 0.2333, 0.20, 0.1667];
const UNCOMMON: [f64; 4] = [0.11, 0.13, 0.17, 0.20];
const RARE: [f64; 4] = [0.02, 0.04, 0.06, 0.10];

fn reward(
    name: &str,
    chances: [f64; 4],
    platinum: Option<u32>,
    ducats: Option<u32>,
) -> PlannedReward {
    PlannedReward {
        name: name.to_owned(),
        chances,
        platinum,
        ducats,
        owned: 0,
    }
}

/// Commons at 5, 5 and 10p, uncommons at 15 and 25p, the rare at 60p: the brief's worked example.
fn relic(name: &str, owned: [u32; 4]) -> PlannedRelic {
    PlannedRelic {
        name: name.to_owned(),
        vaulted: false,
        owned,
        rewards: vec![
            reward("Common A", COMMON, Some(5), Some(15)),
            reward("Common B", COMMON, Some(5), Some(15)),
            reward("Common C", COMMON, Some(10), Some(15)),
            reward("Uncommon A", UNCOMMON, Some(15), Some(45)),
            reward("Uncommon B", UNCOMMON, Some(25), Some(45)),
            reward("Rare", RARE, Some(60), Some(100)),
        ],
    }
}

/// A relic whose commons sell for more than its rare, so refining it loses value.
fn inverted(name: &str, owned: [u32; 4]) -> PlannedRelic {
    PlannedRelic {
        name: name.to_owned(),
        vaulted: true,
        owned,
        rewards: vec![
            reward("Common A", COMMON, Some(30), None),
            reward("Common B", COMMON, Some(30), None),
            reward("Common C", COMMON, Some(30), None),
            reward("Uncommon A", UNCOMMON, Some(5), None),
            reward("Uncommon B", UNCOMMON, Some(5), None),
            reward("Forma", RARE, None, None),
        ],
    }
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 0.01
}

fn draws(refinement: Refinement) -> Vec<(f64, f64)> {
    relic("Axi A1", [0; 4])
        .rewards
        .iter()
        .map(|reward| {
            (
                reward.chances[refinement.index()],
                f64::from(reward.platinum.unwrap()),
            )
        })
        .collect()
}

#[test]
fn one_player_is_worth_the_weighted_average() {
    assert!(close(expected_best(&draws(Refinement::Radiant), 1), 17.33));
    assert!(close(expected_best(&draws(Refinement::Intact), 1), 10.67));
}

#[test]
fn a_shared_run_is_worth_the_best_of_the_squads_draws() {
    assert!(close(expected_best(&draws(Refinement::Radiant), 4), 34.26));
    assert!(close(expected_best(&draws(Refinement::Intact), 4), 19.99));
}

#[test]
fn a_squad_never_lowers_the_value_and_never_passes_the_best_reward() {
    let draws = draws(Refinement::Flawless);
    let mut previous = 0.0;
    for squad in 1..=4 {
        let value = expected_best(&draws, squad);
        assert!(value >= previous);
        assert!(value <= 60.0);
        previous = value;
    }
}

#[test]
fn an_empty_table_is_worth_nothing() {
    assert_eq!(expected_best(&[], 4), 0.0);
    assert_eq!(expected_best(&[(0.5, 10.0)], 0), 0.0);
}

#[test]
fn refinement_costs_are_the_difference_to_radiant() {
    assert_eq!(Refinement::Intact.traces_to_radiant(), 100);
    assert_eq!(Refinement::Exceptional.traces_to_radiant(), 75);
    assert_eq!(Refinement::Flawless.traces_to_radiant(), 50);
    assert_eq!(Refinement::Radiant.traces_to_radiant(), 0);
}

#[test]
fn a_row_carries_the_value_at_every_refinement_and_counts_its_gaps() {
    let plan = plan(&[inverted("Lith B1", [1, 0, 0, 0])], Currency::Platinum, 1);
    let row = &plan.rows[0];
    assert!(row.value[Refinement::Intact.index()] > row.value[Refinement::Radiant.index()]);
    assert_eq!(row.unvalued, 1);
}

#[test]
fn every_held_refinement_is_its_own_entry_priced_by_what_finishing_it_costs() {
    let plan = plan(&[relic("Axi A1", [3, 0, 1, 2])], Currency::Platinum, 4);
    let entries: Vec<_> = plan
        .entries
        .iter()
        .map(|entry| (entry.refinement, entry.copies))
        .collect();
    assert_eq!(
        entries,
        vec![
            (Refinement::Intact, 3),
            (Refinement::Flawless, 1),
            (Refinement::Radiant, 2),
        ]
    );
    // Intact gains 14.27 for 100 traces. Flawless gains 5.88 for 50, which is 11.76 per 100:
    // counted per copy it would look less than half as good.
    let intact = &plan.entries[0];
    assert!(close(intact.value, 19.99));
    assert!(close(intact.radiant_value, 34.26));
    assert!(close(intact.gain_per_100.unwrap(), 14.27));
    assert!(close(plan.entries[1].gain_per_100.unwrap(), 11.76));
    assert_eq!(plan.entries[2].gain_per_100, None);
}

#[test]
fn a_relic_not_held_is_listed_once_at_intact() {
    let plan = plan(
        &[relic("Axi A1", [0; 4]), inverted("Lith B1", [0; 4])],
        Currency::Platinum,
        1,
    );
    assert_eq!(plan.entries.len(), 2);
    assert!(
        plan.entries
            .iter()
            .all(|entry| entry.copies == 0 && entry.refinement == Refinement::Intact)
    );
    // Refining the relic whose commons sell best loses value, so its gain is negative.
    assert!(plan.entries[1].gain_per_100.unwrap() < 0.0);
}

#[test]
fn ducats_are_planned_on_their_own_values() {
    let plan = plan(&[relic("Axi A1", [1, 0, 0, 0])], Currency::Ducats, 1);
    let row = &plan.rows[0];
    // 0.7599 × 15 + 0.22 × 45 + 0.02 × 100, normalised over the published 99.99%.
    assert!(close(row.value[Refinement::Intact.index()], 23.30));
    assert!(close(row.value[Refinement::Radiant.index()], 35.50));
    assert_eq!(row.unvalued, 0);
}

#[test]
fn the_squad_is_held_to_what_the_game_allows() {
    let plan = plan(&[relic("Axi A1", [1, 0, 0, 0])], Currency::Platinum, 9);
    assert_eq!(plan.squad, 4);
}
