use warframe_acquisition::{CatalogIndex, RelicDropTable, RelicRewardIndex};

#[test]
fn loaded_projection_paths_resolve_to_deduplicated_reward_needles() {
    let relics = RelicRewardIndex::from_wfcd_json(
        br#"[
          {"uniqueName":"/Lotus/Types/Game/Projections/TestABronze","name":"Lith A1 Intact","rewards":[
            {"item":{"name":"Perigale Prime Receiver"}},
            {"item":{"name":"Forma Blueprint"}}
          ]},
          {"uniqueName":"/Lotus/Types/Game/Projections/TestBBronze","name":"Lith B1 Intact","rewards":[
            {"item":{"name":"Perigale Prime Receiver"}},
            {"item":{"name":"Burston Prime Receiver"}}
          ]}
        ]"#,
    )
    .unwrap();
    let catalog = CatalogIndex::from_wfcd_json(
        br#"[
          {"uniqueName":"/Lotus/Types/Recipes/PerigalePrimeReceiver","name":"Perigale Prime Receiver","type":"PrimePart","category":"PrimeParts"},
          {"uniqueName":"/Lotus/Types/Recipes/BurstonPrimeReceiver","name":"Burston Prime Receiver","type":"PrimePart","category":"PrimeParts"},
          {"uniqueName":"/Lotus/Types/Recipes/FormaBlueprint","name":"Forma Blueprint","type":"Blueprint","category":"Blueprints"}
        ]"#,
    )
    .unwrap();

    let candidates = relics.candidates_for_projection_paths(
        &[
            "/Lotus/Types/Game/Projections/TestABronze".into(),
            "/Lotus/Types/Game/Projections/TestBBronze".into(),
        ],
        &catalog,
    );

    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.choice_name())
            .collect::<Vec<_>>(),
        vec![
            "Burston Prime Receiver",
            "Forma Blueprint",
            "Perigale Prime Receiver"
        ]
    );
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.internal_paths().len() == 1)
    );
}

#[test]
fn unrelated_projection_paths_produce_no_candidates() {
    let relics = RelicRewardIndex::from_wfcd_json(br#"[]"#).unwrap();
    let catalog = CatalogIndex::default();

    assert!(
        relics
            .candidates_for_projection_paths(&["/Lotus/Unknown".into()], &catalog)
            .is_empty()
    );
}

#[test]
fn relic_blueprint_names_resolve_catalog_component_names_without_the_suffix() {
    let relics = RelicRewardIndex::from_wfcd_json(
        br#"[{
          "uniqueName":"/Lotus/Types/Game/Projections/WukongTestPlatinum",
          "rewards":[{"item":{"name":"Wukong Prime Neuroptics Blueprint"}}]
        }]"#,
    )
    .unwrap();
    let catalog = CatalogIndex::from_wfcd_json(
        br#"[{
          "uniqueName":"/Lotus/Powersuits/MonkeyKing/WukongPrime",
          "name":"Wukong Prime",
          "components":[{
            "uniqueName":"/Lotus/Types/Recipes/WarframeRecipes/WukongPrimeHelmetComponent",
            "name":"Neuroptics",
            "tradable":true,
            "ducats":15
          }]
        }]"#,
    )
    .unwrap();

    let candidates = relics.candidates_for_projection_paths(
        &["/Lotus/Types/Game/Projections/WukongTestPlatinum".into()],
        &catalog,
    );

    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].choice_name(),
        "Wukong Prime Neuroptics Blueprint"
    );
    assert_eq!(
        candidates[0].internal_paths(),
        [b"/Lotus/Types/Recipes/WarframeRecipes/WukongPrimeHelmetComponent".as_slice()]
    );
}

#[test]
fn relic_quantity_prefix_resolves_the_underlying_catalog_item() {
    let relics = RelicRewardIndex::from_wfcd_json(
        br#"[{
          "uniqueName":"/Lotus/Types/Game/Projections/FormaTestPlatinum",
          "rewards":[{"item":{"name":"2X Forma Blueprint"}}]
        }]"#,
    )
    .unwrap();
    let catalog = CatalogIndex::from_wfcd_json(
        br#"[{
          "uniqueName":"/Lotus/Types/Recipes/Components/FormaBlueprint",
          "name":"Forma Blueprint",
          "type":"Blueprint",
          "category":"Blueprints"
        }]"#,
    )
    .unwrap();

    let candidates = relics.candidates_for_projection_paths(
        &["/Lotus/Types/Game/Projections/FormaTestPlatinum".into()],
        &catalog,
    );

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].choice_name(), "2X Forma Blueprint");
    assert_eq!(
        candidates[0].internal_paths(),
        [b"/Lotus/Types/Recipes/Components/FormaBlueprint".as_slice()]
    );
}

/// Axi A1 at all four refinements as WFCD publishes them, and Lith T14 at Intact alone.
const AXI_A1: &[u8] = include_bytes!("fixtures/relics-axi-a1.json");

#[test]
fn drop_tables_keep_every_refinements_chances_and_paths() {
    let table = RelicDropTable::from_wfcd_json(AXI_A1).unwrap();
    let [axi] = table.relics() else {
        panic!("expected one complete relic, got {:?}", table.relics());
    };
    assert_eq!(axi.name, "Axi A1");
    assert!(axi.vaulted);
    assert_eq!(
        axi.paths,
        [
            "/Lotus/Types/Game/Projections/T4VoidProjectionEBronze",
            "/Lotus/Types/Game/Projections/T4VoidProjectionESilver",
            "/Lotus/Types/Game/Projections/T4VoidProjectionEGold",
            "/Lotus/Types/Game/Projections/T4VoidProjectionEPlatinum",
        ]
    );
    assert_eq!(axi.rewards.len(), 6);
    let (_, rare) = axi
        .rewards
        .iter()
        .find(|(name, _)| name == "Nikana Prime Blueprint")
        .unwrap();
    assert_chances(*rare, [0.02, 0.04, 0.06, 0.10]);
    let (_, common) = axi
        .rewards
        .iter()
        .find(|(name, _)| name == "Braton Prime Stock")
        .unwrap();
    assert_chances(*common, [0.2533, 0.2333, 0.20, 0.1667]);
}

/// Chances are published in percent and stored as fractions, so the division leaves float dust.
fn assert_chances(actual: [f64; 4], expected: [f64; 4]) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "{actual} is not {expected}"
        );
    }
}

#[test]
fn a_relic_missing_a_refinement_is_dropped_rather_than_ranked() {
    let table = RelicDropTable::from_wfcd_json(AXI_A1).unwrap();
    assert!(table.relics().iter().all(|relic| relic.name != "Lith T14"));
}

#[test]
fn a_table_whose_chances_do_not_add_up_is_dropped() {
    let mut records: Vec<serde_json::Value> = serde_json::from_slice(AXI_A1).unwrap();
    records[0]["rewards"][0]["chance"] = serde_json::json!(50);
    let bytes = serde_json::to_vec(&records).unwrap();
    assert!(
        RelicDropTable::from_wfcd_json(&bytes)
            .unwrap()
            .relics()
            .is_empty()
    );
}

#[test]
fn a_reward_listed_twice_keeps_both_chances() {
    let mut records: Vec<serde_json::Value> = serde_json::from_slice(AXI_A1).unwrap();
    for record in &mut records {
        record["rewards"][0]["item"]["name"] = serde_json::json!("2X Forma Blueprint");
        record["rewards"][1]["item"]["name"] = serde_json::json!("2X Forma Blueprint");
    }
    let bytes = serde_json::to_vec(&records).unwrap();
    let table = RelicDropTable::from_wfcd_json(&bytes).unwrap();
    let [axi] = table.relics() else {
        panic!("the relic should survive a repeated reward");
    };
    let forma: Vec<_> = axi
        .rewards
        .iter()
        .filter(|(name, _)| name == "2X Forma Blueprint")
        .collect();
    assert_eq!(forma.len(), 2);
}

/// Lith B12 as `api.warframestat.us` served it on 2026-10-26 (descriptions and market ids cut).
/// It was missing from the Relics tab because the app read an older pinned revision of WFCD's
/// `Relics.json` that predates it.
const LITH_B12: &[u8] = include_bytes!("fixtures/relics-lith-b12.json");

#[test]
fn a_relic_from_the_live_catalog_is_ranked() {
    let table = RelicDropTable::from_wfcd_json(LITH_B12).unwrap();
    let [lith] = table.relics() else {
        panic!("Lith B12 should be in the table");
    };
    assert_eq!(lith.name, "Lith B12");
    assert!(lith.vaulted);
    assert_eq!(
        lith.paths[0],
        "/Lotus/Types/Game/Projections/T1VoidProjectionProteaIvaraVaultABronze"
    );
    assert_eq!(lith.rewards.len(), 6);
    let (_, baza) = lith
        .rewards
        .iter()
        .find(|(name, _)| name == "Baza Prime Stock")
        .unwrap();
    assert_eq!(*baza, [0.02, 0.04, 0.06, 0.10]);
}
