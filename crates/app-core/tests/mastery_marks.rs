use std::sync::Arc;

use app_core::AppCore;
use local_store::SnapshotMeta;
use warframe_acquisition::{
    AcquisitionError, AcquisitionFailure, AcquisitionHealth, AcquisitionResult, CatalogIndex,
    MasteryEvidence, MasteryFacts,
};
use warframe_domain::{
    CatalogItem, Category, InventoryEntry, InventorySnapshot, ItemId, MasteryMark,
};

const GUANDAO: &str = "/Lotus/Weapons/Tenno/Melee/Polearms/GuandaoPrime";
const GUANDAO_JSON: &str = r#"[{"uniqueName": "/Lotus/Weapons/Tenno/Melee/Polearms/GuandaoPrime",
  "name": "Guandao Prime", "category": "Melee", "type": "Melee", "masterable": true,
  "tradable": false, "masteryReq": 12, "maxLevelCap": 30, "components": [
  {"uniqueName": "/Lotus/Types/Recipes/Weapons/GuandaoPrimeBlueprint", "name": "Blueprint",
   "itemCount": 1, "tradable": true, "ducats": 45, "imageName": "blueprint.png"},
  {"uniqueName": "/Lotus/Types/Recipes/Weapons/WeaponParts/GuandaoPrimeBlade", "name": "Blade",
   "itemCount": 2, "tradable": true, "ducats": 15, "imageName": "GenericWeaponPrimeBlade.png"},
  {"uniqueName": "/Lotus/Types/Recipes/Weapons/WeaponParts/GuandaoPrimeHandle", "name": "Handle",
   "itemCount": 1, "tradable": true, "ducats": 100, "imageName": "GenericWeaponPrimeHandle.png"}]}]"#;

fn guandao_catalog() -> CatalogIndex {
    CatalogIndex::from_wfcd_json(GUANDAO_JSON.as_bytes()).unwrap()
}

fn mastered_guandao() -> InventoryEntry {
    InventoryEntry::new(
        CatalogItem::new(
            ItemId::new(GUANDAO).unwrap(),
            "Guandao Prime",
            Category::Weapon,
        )
        .unwrap(),
        0,
    )
    .with_mastered(true)
}

fn saved_core() -> AppCore {
    let mut core = AppCore::in_memory().unwrap();
    core.apply_inventory_snapshot(
        InventorySnapshot::coherent(vec![mastered_guandao()]).unwrap(),
        SnapshotMeta::fake("saved").unwrap(),
    )
    .unwrap();
    core
}

fn live_refresh() -> (AcquisitionResult, SnapshotMeta) {
    let result = AcquisitionResult::new(
        InventorySnapshot::coherent(vec![mastered_guandao()]).unwrap(),
        AcquisitionHealth::successful(),
    )
    .unwrap()
    .with_mastery_facts(MasteryFacts::default());
    (result, SnapshotMeta::fake("live").unwrap())
}

#[test]
fn a_core_without_a_catalog_marks_nothing() {
    let core = saved_core();
    assert_eq!(
        core.mastery_view()
            .unwrap()
            .reward_mark("Guandao Prime Blade"),
        None
    );
}

#[test]
fn a_saved_collection_is_saved_evidence_and_live_facts_upgrade_it() {
    let mut core = saved_core();
    core.set_mastery_catalog(Arc::new(guandao_catalog()));
    let view = core.mastery_view().unwrap();
    assert_eq!(view.evidence(), MasteryEvidence::Saved);
    assert_eq!(
        view.reward_mark("Guandao Prime Blade"),
        Some(MasteryMark::Mastered)
    );

    core.set_mastery_facts(MasteryFacts::default());
    assert_eq!(
        core.mastery_view().unwrap().evidence(),
        MasteryEvidence::Live
    );

    core.clear_mastery_facts();
    assert_eq!(
        core.mastery_view().unwrap().evidence(),
        MasteryEvidence::Saved
    );
}

#[test]
fn a_successful_inventory_refresh_makes_the_evidence_live() {
    let mut core = saved_core();
    core.set_mastery_catalog(Arc::new(guandao_catalog()));
    core.finish_inventory_refresh(Ok(live_refresh()), None)
        .unwrap();
    assert_eq!(
        core.mastery_view().unwrap().evidence(),
        MasteryEvidence::Live
    );
}

#[test]
fn a_failed_refresh_keeps_the_last_good_facts() {
    let mut core = saved_core();
    core.finish_inventory_refresh(Ok(live_refresh()), None)
        .unwrap();
    let failure = AcquisitionFailure::for_test(AcquisitionError::InventoryRequestFailed);
    core.finish_inventory_refresh(Err(failure), None).unwrap();
    assert_eq!(
        core.mastery_view().unwrap().evidence(),
        MasteryEvidence::Live
    );
}

/// A part is never mastered itself. The collection marks it through the item it builds, so the
/// Mastered filter can show the parts that are safe to let go.
#[test]
fn a_part_of_a_mastered_item_reads_item_mastered_in_the_collection() {
    let blade = InventoryEntry::new(
        CatalogItem::new(
            ItemId::new("/Lotus/Types/Recipes/Weapons/WeaponParts/GuandaoPrimeBlade").unwrap(),
            "Guandao Prime Blade",
            Category::PrimePart,
        )
        .unwrap(),
        2,
    );
    let mut core = AppCore::in_memory().unwrap();
    core.apply_inventory_snapshot(
        InventorySnapshot::coherent(vec![mastered_guandao(), blade.clone()]).unwrap(),
        SnapshotMeta::fake("saved").unwrap(),
    )
    .unwrap();
    let blade_of = |core: &AppCore| {
        core.current_view()
            .unwrap()
            .collection()
            .items()
            .iter()
            .find(|item| item.name() == "Guandao Prime Blade")
            .map(|item| (item.mastered(), item.item_mastered()))
    };
    assert_eq!(
        blade_of(&core),
        Some((false, false)),
        "no catalog, no claim"
    );

    core.set_mastery_catalog(Arc::new(guandao_catalog()));
    assert_eq!(blade_of(&core), Some((false, true)));

    let mut unmastered = AppCore::in_memory().unwrap();
    unmastered
        .apply_inventory_snapshot(
            InventorySnapshot::coherent(vec![blade]).unwrap(),
            SnapshotMeta::fake("saved").unwrap(),
        )
        .unwrap();
    unmastered.set_mastery_catalog(Arc::new(guandao_catalog()));
    assert_eq!(blade_of(&unmastered), Some((false, false)));
}
