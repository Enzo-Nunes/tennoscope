use app_lib::{RewardLogEvent, RewardLogMachine};

#[test]
fn reward_window_activity_tracks_open_and_close() {
    let mut machine = RewardLogMachine::default();
    assert!(!machine.reward_window_open());

    machine.observe_line("VoidProjections: OpenVoidProjectionRewardScreenRMI");
    assert!(machine.reward_window_open());

    machine.observe_line("ProjectionRewardChoice.lua: Relic reward screen shut down");
    assert!(!machine.reward_window_open());
}

#[test]
fn online_sequence_requests_one_baseline_and_emits_the_observed_choice_count() {
    let mut machine = RewardLogMachine::default();
    let lines = [
        "Resource load completed (/Lotus/Types/Game/Projections/T1VoidProjectionStyanaxPrimeDBronze)",
        "Resource load completed (/Lotus/Types/Game/Projections/T1VoidProjectionGyrePrimeDBronze)",
        "VoidProjections: OpenVoidProjectionRewardScreenRMI",
        "VoidProjections: Client got reward info from player-a",
        "VoidProjections: Still waiting on response from player-b",
        "VoidProjections: Client got reward info from player-b",
        "VoidProjections: Client got reward info from player-c",
        "VoidProjections: Client got reward info from player-d",
        "VoidProjections: Client has reward info for all players now",
        "ProjectionRewardChoice.lua: Got rewards",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionsCountdown.lua: Initialize timer nil 15",
    ];
    let events = lines
        .into_iter()
        .flat_map(|line| machine.observe_line(line))
        .filter(is_render_lifecycle_event)
        .collect::<Vec<_>>();

    assert_eq!(
        events,
        vec![
            RewardLogEvent::BaselineRequested {
                relic_paths: vec![
                    "/Lotus/Types/Game/Projections/T1VoidProjectionStyanaxPrimeDBronze".into(),
                    "/Lotus/Types/Game/Projections/T1VoidProjectionGyrePrimeDBronze".into(),
                ],
            },
            RewardLogEvent::ChoicesReady {
                expected_choices: 4,
                local_reward_path: None,
            },
        ]
    );
}

#[test]
fn multi_player_relic_paths_request_a_baseline_before_the_reward_window() {
    let mut machine = RewardLogMachine::default();

    assert!(
        machine
            .observe_line("loaded (/Lotus/Types/Game/Projections/First)")
            .is_empty()
    );
    assert_eq!(
        machine.observe_line("loaded (/Lotus/Types/Game/Projections/Second)"),
        vec![RewardLogEvent::BaselineRequested {
            relic_paths: vec![
                "/Lotus/Types/Game/Projections/First".into(),
                "/Lotus/Types/Game/Projections/Second".into(),
            ],
        }]
    );
    assert_eq!(
        machine.observe_line("VoidProjections: OpenVoidProjectionRewardScreenRMI"),
        vec![RewardLogEvent::RewardWindowOpened]
    );
}

#[test]
fn rendered_card_count_overrides_the_number_of_network_responders() {
    let mut machine = RewardLogMachine::default();
    let events = [
        "VoidProjections: OpenVoidProjectionRewardScreenRMI",
        "VoidProjections: Client got reward info from player-a",
        "VoidProjections: Client got reward info from player-b",
        "VoidProjections: Client got reward info from player-c",
        "VoidProjections: Client got reward info from player-d",
        "VoidProjections: Client has reward info for all players now",
        "ProjectionRewardChoice.lua: Got rewards",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionsCountdown.lua: Initialize timer nil 15",
    ]
    .into_iter()
    .flat_map(|line| machine.observe_line(line))
    .filter(is_render_lifecycle_event)
    .collect::<Vec<_>>();

    assert_eq!(
        events,
        vec![RewardLogEvent::ChoicesReady {
            expected_choices: 3,
            local_reward_path: None,
        }]
    );
}

#[test]
fn host_sequence_emits_the_rendered_choice_count() {
    let mut machine = RewardLogMachine::default();
    let events = [
        "VoidProjections: OpenVoidProjectionRewardScreen - PostMigration: 0",
        "VoidProjections: Host got reward info from player-a",
        "VoidProjections: Host got reward info from player-b",
        "VoidProjections: Host got reward info from player-c",
        "VoidProjections: Host got reward info from player-d",
        "VoidProjections: Host has reward info for all players now!",
        "ProjectionRewardChoice.lua: Got rewards",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionsCountdown.lua: Initialize timer nil 15",
    ]
    .into_iter()
    .flat_map(|line| machine.observe_line(line))
    .filter(is_render_lifecycle_event)
    .collect::<Vec<_>>();

    assert_eq!(
        events,
        vec![RewardLogEvent::ChoicesReady {
            expected_choices: 4,
            local_reward_path: None,
        }]
    );
}

#[test]
fn choices_include_the_explicitly_logged_local_reward_path() {
    let mut machine = RewardLogMachine::default();
    let events = [
        "VoidProjections: OpenVoidProjectionRewardScreenRMI",
        "VoidProjections: player gets reward /Lotus/Types/Recipes/Weapons/BratonPrimeBlueprint",
        "VoidProjections: Client got reward info from player-a",
        "VoidProjections: Client got reward info from player-b",
        "VoidProjections: Client has reward info for all players now",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionRewardChoice.lua: Missing icon data!",
        "ProjectionsCountdown.lua: Initialize timer nil 15",
    ]
    .into_iter()
    .flat_map(|line| machine.observe_line(line))
    .filter(is_render_lifecycle_event)
    .collect::<Vec<_>>();

    assert_eq!(
        events,
        vec![RewardLogEvent::ChoicesReady {
            expected_choices: 2,
            local_reward_path: Some("/Lotus/Types/Recipes/Weapons/BratonPrimeBlueprint".into()),
        }]
    );
}

#[test]
fn solo_reward_without_a_choice_screen_never_requests_memory_scanning() {
    let mut machine = RewardLogMachine::default();
    let events = [
        "Resource load completed (/Lotus/Types/Game/Projections/T1VoidProjectionStyanaxPrimeCBronze)",
        "VoidProjections: player gets reward /Lotus/StoreItems/Types/Recipes/Weapons/Test",
    ]
    .into_iter()
    .flat_map(|line| machine.observe_line(line))
    .collect::<Vec<_>>();

    assert!(events.is_empty());
}

#[test]
fn shutdown_closes_and_resets_the_reward_window() {
    let mut machine = RewardLogMachine::default();
    machine.observe_line("Resource load completed (/Lotus/Types/Game/Projections/First)");
    machine.observe_line("VoidProjections: OpenVoidProjectionRewardScreenRMI");

    assert_eq!(
        machine.observe_line("ProjectionRewardChoice.lua: Relic reward screen shut down"),
        vec![RewardLogEvent::Closed]
    );
    assert_eq!(
        machine.observe_line("VoidProjections: OpenVoidProjectionRewardScreenRMI"),
        vec![RewardLogEvent::RewardWindowOpened]
    );
}

#[test]
fn byte_stream_preserves_lines_split_across_monitor_reads() {
    let mut machine = RewardLogMachine::default();

    assert!(
        machine
            .observe_bytes(b"OpenVoidProjectionReward")
            .is_empty()
    );
    let events = machine.observe_bytes(b"ScreenRMI\n");

    assert_eq!(events, vec![RewardLogEvent::RewardWindowOpened]);
}

#[test]
fn live_response_sequence_emits_ordered_responders_and_completes_before_rendering() {
    let mut machine = RewardLogMachine::default();
    let events = [
        "14189.789 Sys [Info]: VoidProjections: OpenVoidProjectionRewardScreenRMI",
        "14189.989 Sys [Info]: VoidProjections: Client got reward info from de1e7ed00000000000000005",
        "14190.058 Sys [Info]: VoidProjections: Client got reward info from de1e7ed0000000000000000a",
        "14190.062 Sys [Info]: VoidProjections: Client got reward info from de1e7ed00000000000000004",
        "14190.095 Sys [Info]: VoidProjections: de1e7ed00000000000000006 gets reward /Lotus/StoreItems/Types/Recipes/Weapons/WeaponParts/PrimeDaikyuUpperLimb",
        "14190.095 Sys [Info]: VoidProjections: Client got reward info from de1e7ed00000000000000006",
        "14190.336 Sys [Info]: VoidProjections: Client has reward info for all players now",
    ]
    .into_iter()
    .flat_map(|line| machine.observe_line(line))
    .collect::<Vec<_>>();

    assert_eq!(
        events,
        vec![
            RewardLogEvent::RewardWindowOpened,
            RewardLogEvent::ResponderReceived {
                identity: "de1e7ed00000000000000005".into(),
                is_local: false,
            },
            RewardLogEvent::ResponderReceived {
                identity: "de1e7ed0000000000000000a".into(),
                is_local: false,
            },
            RewardLogEvent::ResponderReceived {
                identity: "de1e7ed00000000000000004".into(),
                is_local: false,
            },
            RewardLogEvent::ResponderReceived {
                identity: "de1e7ed00000000000000006".into(),
                is_local: true,
            },
            RewardLogEvent::ResponsesComplete {
                responders: vec![
                    "de1e7ed00000000000000005".into(),
                    "de1e7ed0000000000000000a".into(),
                    "de1e7ed00000000000000004".into(),
                    "de1e7ed00000000000000006".into(),
                ],
                screen_order: vec![
                    "de1e7ed00000000000000006".into(),
                    "de1e7ed00000000000000005".into(),
                    "de1e7ed0000000000000000a".into(),
                    "de1e7ed00000000000000004".into(),
                ],
                local_reward_path: Some(
                    "/Lotus/StoreItems/Types/Recipes/Weapons/WeaponParts/PrimeDaikyuUpperLimb"
                        .into(),
                ),
                local_identity: Some("de1e7ed00000000000000006".into()),
            },
        ]
    );
}

#[test]
fn opening_reward_screen_initializes_candidates_for_one_unique_relic() {
    let mut machine = RewardLogMachine::default();
    let relic = "/Lotus/Types/Game/Projections/LithA1Bronze";

    assert!(
        machine
            .observe_line(&format!("LoadResource {relic}"))
            .is_empty()
    );

    assert_eq!(
        machine.observe_line("VoidProjections: OpenVoidProjectionRewardScreenRMI"),
        vec![
            RewardLogEvent::RewardWindowOpened,
            RewardLogEvent::BaselineRequested {
                relic_paths: vec![relic.into()],
            },
        ]
    );
}

#[test]
fn waiting_list_ring_rotates_to_the_local_players_screen_order() {
    let mut machine = RewardLogMachine::default();
    let complete = [
        "VoidProjections: OpenVoidProjectionRewardScreenRMI",
        "VoidProjections: Client got reward info from de1e7ed00000000000000010",
        "VoidProjections: Still waiting on response from de1e7ed0000000000000000f",
        "VoidProjections: Still waiting on response from de1e7ed00000000000000006",
        "VoidProjections: Still waiting on response from de1e7ed00000000000000009",
        "VoidProjections: de1e7ed00000000000000006 gets reward /Lotus/StoreItems/Types/Recipes/Weapons/WeaponParts/BratonPrimeBarrel",
        "VoidProjections: Client got reward info from de1e7ed00000000000000006",
        "VoidProjections: Client got reward info from de1e7ed00000000000000009",
        "VoidProjections: Client got reward info from de1e7ed0000000000000000f",
        "VoidProjections: Client has reward info for all players now",
    ]
    .into_iter()
    .flat_map(|line| machine.observe_line(line))
    .find_map(|event| match event {
        RewardLogEvent::ResponsesComplete { screen_order, .. } => Some(screen_order),
        _ => None,
    })
    .unwrap();

    assert_eq!(
        complete,
        vec![
            "de1e7ed00000000000000006",
            "de1e7ed00000000000000009",
            "de1e7ed00000000000000010",
            "de1e7ed0000000000000000f",
        ]
    );
}

#[test]
fn waiting_responder_is_exposed_before_its_reward_arrives() {
    let mut machine = RewardLogMachine::default();
    machine.observe_line("VoidProjections: OpenVoidProjectionRewardScreenRMI");

    assert_eq!(
        machine.observe_line(
            "VoidProjections: Still waiting on response from de1e7ed00000000000000003"
        ),
        vec![RewardLogEvent::ResponderExpected {
            identity: "de1e7ed00000000000000003".into(),
        }]
    );
    assert!(
        machine
            .observe_line(
                "VoidProjections: Still waiting on response from de1e7ed00000000000000003"
            )
            .is_empty()
    );
}

// Real lines from the 2026-10-29 Void Cascade, where a dev rebuild restarted the app at
// 20:28:48, after this round's relics loaded (20:24:20) and before its screen opened (20:29:13).
const CASCADE_FIRST_SHUTDOWN: &str =
    "1235.999 Script [Info]: ProjectionRewardChoice.lua: Relic reward screen shut down";
const CASCADE_RELIC_LOADS: [&str; 4] = [
    "1238.556 Sys [Info]: Resource load completed 0x000000005B9054D0 (/Lotus/Types/Game/Projections/T3VoidProjectionCitrinePrimeBBronze) in one pass and 0.0s (I/O ~= 0.0%, inherited 20 of 20)",
    "1239.469 Sys [Info]: Resource load completed 0x0000000047E51160 (/Lotus/Types/Game/Projections/T2VoidProjectionProteaPrimeDPlatinum) in one pass and 0.0s (I/O ~= 0.0%, inherited 19 of 19)",
    "1240.117 Sys [Info]: Resource load completed 0x0000000061E7E480 (/Lotus/Types/Game/Projections/T2VoidProjectionRevenantPrimeBPlatinum) in one pass and 0.0s (I/O ~= 0.0%, inherited 16 of 16)",
    "1243.490 Sys [Info]: Resource load completed 0x0000000030FB22D0 (/Lotus/Types/Game/Projections/T3VoidProjectionLavosPrimeDPlatinum) in one pass and 0.0s (I/O ~= 0.0%, inherited 18 of 18)",
];
// The game's second shutdown for the same screen, logged after the next round's relics loaded.
const CASCADE_FORCE_CLOSE: &str =
    "1248.469 Script [Info]: ProjectionRewardChoice.lua: Relic reward screen shut down";
const CASCADE_OPEN: &str =
    "1531.786 Sys [Info]: VoidProjections: OpenVoidProjectionRewardScreenRMI";
const CASCADE_SHUTDOWN: &str =
    "1552.447 Script [Info]: ProjectionRewardChoice.lua: Relic reward screen shut down";

fn cascade_tail(lines: &[&str]) -> Vec<u8> {
    let mut bytes = b"ded 0x0000000047757680 (/Lotus/Interface/Icons/CosmeticEnhancers/Arcanes/Projections/ArcaneProjectionBZ.png) starting\n".to_vec();
    for line in lines {
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

fn cascade_relics() -> Vec<String> {
    [
        "T3VoidProjectionCitrinePrimeBBronze",
        "T2VoidProjectionProteaPrimeDPlatinum",
        "T2VoidProjectionRevenantPrimeBPlatinum",
        "T3VoidProjectionLavosPrimeDPlatinum",
    ]
    .into_iter()
    .map(|name| format!("/Lotus/Types/Game/Projections/{name}"))
    .collect()
}

#[test]
fn adopted_tail_with_loaded_relics_and_no_shutdown_requests_their_baseline() {
    let mut lines = vec![CASCADE_FIRST_SHUTDOWN];
    lines.extend(CASCADE_RELIC_LOADS);
    lines.push(CASCADE_FORCE_CLOSE);
    let mut machine = RewardLogMachine::default();

    assert_eq!(
        machine.adopt_tail(&cascade_tail(&lines)),
        vec![RewardLogEvent::BaselineRequested {
            relic_paths: cascade_relics(),
        }]
    );
    assert!(!machine.reward_window_open());
}

#[test]
fn adopted_tail_ending_in_a_shutdown_arms_nothing() {
    let mut lines = CASCADE_RELIC_LOADS.to_vec();
    lines.extend([CASCADE_OPEN, CASCADE_SHUTDOWN]);
    let mut machine = RewardLogMachine::default();

    assert!(machine.adopt_tail(&cascade_tail(&lines)).is_empty());
    assert!(!machine.reward_window_open());
    assert_eq!(
        machine.observe_line(CASCADE_OPEN),
        vec![RewardLogEvent::RewardWindowOpened]
    );
}

#[test]
fn adopted_tail_ignores_its_partial_first_line() {
    // The fragment is the tail of a relic load the read started inside of; only the complete
    // load after it counts, so one relic alone does not request a baseline yet.
    let mut bytes = CASCADE_RELIC_LOADS[0].as_bytes()[40..].to_vec();
    bytes.push(b'\n');
    bytes.extend_from_slice(CASCADE_RELIC_LOADS[1].as_bytes());
    bytes.push(b'\n');
    let mut machine = RewardLogMachine::default();

    assert!(machine.adopt_tail(&bytes).is_empty());
    assert_eq!(
        machine.observe_line(CASCADE_OPEN),
        vec![
            RewardLogEvent::RewardWindowOpened,
            RewardLogEvent::BaselineRequested {
                relic_paths: vec![
                    "/Lotus/Types/Game/Projections/T2VoidProjectionProteaPrimeDPlatinum".into()
                ],
            },
        ]
    );
}

#[test]
fn reward_screen_after_an_adopted_tail_opens_without_a_second_baseline() {
    let mut machine = RewardLogMachine::default();
    assert_eq!(
        machine
            .adopt_tail(&cascade_tail(&CASCADE_RELIC_LOADS))
            .len(),
        1
    );

    assert_eq!(
        machine.observe_line(CASCADE_OPEN),
        vec![RewardLogEvent::RewardWindowOpened]
    );
    assert!(machine.reward_window_open());
}

fn is_render_lifecycle_event(event: &RewardLogEvent) -> bool {
    matches!(
        event,
        RewardLogEvent::BaselineRequested { .. } | RewardLogEvent::ChoicesReady { .. }
    )
}
