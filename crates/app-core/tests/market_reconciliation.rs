use app_core::{OrderStatus, reconcile_orders};
use local_store::{SnapshotInstant, SnapshotMeta};
use warframe_acquisition::CatalogIndex;
use warframe_domain::{
    CatalogItem, Category, Collection, InventoryEntry, InventorySnapshot, ItemId,
};
use warframe_market::{MarketItems, MarketOrder, OrderKind};

const BRATON_PATH: &str = "/Lotus/Types/Recipes/Weapons/BratonPrimeBlueprint";
const BRATON_ID: &str = "54a73e65e779893a797fff33";
const MOD_PATH: &str = "/Lotus/Upgrades/Mods/Pistol/DualStat/CorruptedCritChanceFireRatePistol";
const MOD_ID: &str = "54ca39abe7798915c1c11e10";
/// A market item with no `gameRef`, of which the live table carries 35.
const RETIRED_ID: &str = "5program0000000000000000";
/// A relic and a set, both taken verbatim from `GET /v2/items` on 2026-08-03. Their published
/// paths name rows the collection never carries. The relic's four refinements are stored
/// suffixed, and a set's parts are `/Recipes/` rows rather than the built item, which is why
/// neither can be compared against an owned quantity.
const RELIC_ID: &str = "56783f24cbfa8f0432dd89a2";
const SET_ID: &str = "54a73e65e779893a797ffef1";

const ITEMS: &str = r#"{"apiVersion":"0.25.0","data":[
    {"id":"54a73e65e779893a797fff33",
     "gameRef":"/Lotus/Types/Recipes/Weapons/BratonPrimeBlueprint",
     "i18n":{"en":{"name":"Braton Prime Blueprint"}}},
    {"id":"54ca39abe7798915c1c11e10",
     "gameRef":"/Lotus/Upgrades/Mods/Pistol/DualStat/CorruptedCritChanceFireRatePistol",
     "maxRank":5,
     "i18n":{"en":{"name":"Creeping Bullseye"}}},
    {"id":"56783f24cbfa8f0432dd89a2",
     "gameRef":"/Lotus/Types/Game/Projections/T4VoidProjectionE",
     "subtypes":["intact","exceptional","flawless","radiant"],
     "i18n":{"en":{"name":"Axi A1 Relic"}}},
    {"id":"54a73e65e779893a797ffef1",
     "gameRef":"/Lotus/Weapons/Tenno/Rifle/BratonPrime",
     "i18n":{"en":{"name":"Braton Prime Set"}}},
    {"id":"5program0000000000000000","i18n":{"en":{"name":"Legendary Fusion Core"}}}
],"error":null}"#;

fn items() -> MarketItems {
    MarketItems::from_response(ITEMS.as_bytes()).expect("items parse")
}

fn collection_holding(entries: Vec<(&str, u32, Option<u32>)>) -> Collection {
    let entries = entries
        .into_iter()
        .map(|(path, quantity, rank)| {
            let id = match rank {
                Some(rank) => ItemId::new(format!("{path}#{rank}")).expect("item id"),
                None => ItemId::new(path).expect("item id"),
            };
            let item = CatalogItem::new(id, "Item", Category::PrimePart).expect("catalog item");
            let entry = InventoryEntry::new(item, quantity);
            match rank {
                Some(rank) => entry.with_rank(rank, Some(5)),
                None => entry,
            }
        })
        .collect::<Vec<_>>();
    let mut collection = Collection::default();
    collection.replace(InventorySnapshot::coherent(entries).expect("snapshot"));
    collection
}

fn snapshot_at(seconds: i64) -> SnapshotMeta {
    SnapshotMeta::new(
        SnapshotInstant::from_unix_seconds(seconds).unwrap(),
        "build-for-test".to_owned(),
        "test-fixture-source".to_owned(),
    )
    .expect("snapshot meta")
}

fn sell_order(item_id: &str, quantity: u32, updated_at: i64) -> MarketOrder {
    MarketOrder {
        id: "order-under-test".to_owned(),
        item_id: item_id.to_owned(),
        kind: OrderKind::Sell,
        platinum: 12,
        quantity,
        per_trade: 1,
        rank: None,
        subtype: None,
        visible: true,
        updated_at: Some(SnapshotInstant::from_unix_seconds(updated_at).unwrap()),
    }
}

#[test]
fn an_order_backed_by_the_collection_is_ok() {
    let orders = vec![sell_order(BRATON_ID, 2, 1_785_405_600)];

    let reconciled = reconcile_orders(
        &orders,
        &items(),
        &collection_holding(vec![(BRATON_PATH, 3, None)]),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Ok);
}

/// The case the feature exists for: sold in game, never taken down.
#[test]
fn an_order_for_something_unowned_is_missing() {
    let orders = vec![sell_order(BRATON_ID, 1, 1_785_405_600)];

    let reconciled = reconcile_orders(
        &orders,
        &items(),
        &collection_holding(vec![(MOD_PATH, 4, None)]),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Missing);
}

/// A quantity-zero row is not ownership. The collection keeps mastered items at zero, and an order
/// against one is as wrong as an order against an item that is absent entirely.
#[test]
fn an_order_against_a_zero_quantity_row_is_missing() {
    let orders = vec![sell_order(BRATON_ID, 1, 1_785_405_600)];

    let reconciled = reconcile_orders(
        &orders,
        &items(),
        &collection_holding(vec![(BRATON_PATH, 0, None)]),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Missing);
}

#[test]
fn an_order_listing_more_than_is_owned_overshoots() {
    let orders = vec![sell_order(BRATON_ID, 3, 1_785_405_600)];

    let reconciled = reconcile_orders(
        &orders,
        &items(),
        &collection_holding(vec![(BRATON_PATH, 1, None)]),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Overshoot { owned: 1 });
}

/// No snapshot means no opinion. Without this, a fresh installation that has never read the game
/// would flag every order the player holds and offer to delete each one.
#[test]
fn without_a_snapshot_nothing_is_claimed() {
    let orders = vec![sell_order(BRATON_ID, 1, 1_785_405_600)];

    let reconciled = reconcile_orders(&orders, &items(), &Collection::default(), None, None);

    assert_eq!(reconciled[0].status, OrderStatus::Unverifiable);
}

#[test]
fn an_order_newer_than_the_snapshot_is_unverifiable_without_downstream_parsing() {
    let orders = vec![sell_order(MOD_ID, 1, 1_785_492_000)];

    let reconciled = reconcile_orders(
        &orders,
        &items(),
        &collection_holding(vec![(MOD_PATH, 4, None)]),
        Some(&snapshot_at(1_785_405_600)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Unverifiable);
}

/// An order the market publishes no game reference for cannot be looked up at all. 35 of the live
/// table's 3,837 entries are like this.
#[test]
fn an_order_with_no_resolvable_item_is_unverifiable() {
    let orders = vec![sell_order(RETIRED_ID, 1, 1_785_405_600)];

    let reconciled = reconcile_orders(
        &orders,
        &items(),
        &collection_holding(vec![(BRATON_PATH, 1, None)]),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Unverifiable);
    // Still nameable, so the row reads as an order rather than as a blank.
    assert_eq!(reconciled[0].name.as_deref(), Some("Legendary Fusion Core"));
}

/// A ranked order names a copy at a rank, and the collection stores each rank as its own entry.
/// Which copies the order means is not answerable from the order alone.
#[test]
fn a_ranked_order_is_unverifiable() {
    let mut order = sell_order(MOD_ID, 1, 1_785_405_600);
    order.rank = Some(5);

    let reconciled = reconcile_orders(
        &[order],
        &items(),
        &collection_holding(vec![(MOD_PATH, 2, Some(5))]),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Unverifiable);
}

/// Owning none of something is the ordinary state for something you are trying to buy.
#[test]
fn a_buy_order_is_never_reconciled() {
    let mut order = sell_order(BRATON_ID, 1, 1_785_405_600);
    order.kind = OrderKind::Buy;

    let reconciled = reconcile_orders(
        &[order],
        &items(),
        &Collection::default(),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Unverifiable);
}

/// An order with no update time cannot be placed relative to the snapshot, so it is not judged.
#[test]
fn an_order_with_no_update_time_is_unverifiable() {
    let mut order = sell_order(BRATON_ID, 1, 1_785_405_600);
    order.updated_at = None;

    let reconciled = reconcile_orders(
        &[order],
        &items(),
        &collection_holding(vec![(MOD_PATH, 1, None)]),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Unverifiable);
}

/// Quantity is summed across the ranked entries of one card. A player holding two unranked and one
/// at rank 5 holds three, and an order for three is not an overshoot.
#[test]
fn owned_quantity_sums_every_rank_of_one_card() {
    let orders = vec![sell_order(MOD_ID, 3, 1_785_405_600)];

    let reconciled = reconcile_orders(
        &orders,
        &items(),
        &collection_holding(vec![(MOD_PATH, 2, None), (MOD_PATH, 1, Some(5))]),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Ok);
}

/// Production snapshot metadata records Unix seconds while orders carry RFC 3339. Compared as
/// text, every digit sorts below every year, so every order would read as unverifiable forever
/// and the feature would ship doing nothing.
#[test]
fn an_epoch_seconds_snapshot_is_compared_against_an_rfc_3339_order() {
    let orders = vec![sell_order(BRATON_ID, 1, 1_785_405_600)];

    // 2026-07-31T10:00:00Z.
    let reconciled = reconcile_orders(
        &orders,
        &items(),
        &collection_holding(vec![(MOD_PATH, 4, None)]),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Missing);
}

/// A relic listing is not evidence about the collection. warframe.market publishes one entry per
/// relic carrying the base projection path, with the four refinements as subtypes; the collection
/// stores each refinement as its own suffixed row. The base path is a row it never holds, so
/// comparing against it answers "owned: none" for every relic anyone has ever listed.
#[test]
fn a_relic_order_is_unverifiable_rather_than_missing() {
    let orders = vec![sell_order(RELIC_ID, 1, 1_785_405_600)];

    let reconciled = reconcile_orders(
        &orders,
        &items(),
        // The refinement the player actually holds, under the name the collection gives it.
        &collection_holding(vec![(
            "/Lotus/Types/Game/Projections/T4VoidProjectionEBronze",
            3,
            None,
        )]),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(
        reconciled[0].status,
        OrderStatus::Unverifiable,
        "a relic listing must never be offered a delete button on the strength of a path the \
         collection cannot carry"
    );
}

/// A set listing names the built item; what the seller holds is the parts. Left uncaught, every
/// set on the account, the most common thing there is to sell, reads as missing. Before the
/// recipe index has loaded there is nothing to read the parts from, so nothing is claimed.
#[test]
fn a_set_order_is_unverifiable_rather_than_missing() {
    let orders = vec![sell_order(SET_ID, 1, 1_785_405_600)];

    let reconciled = reconcile_orders(
        &orders,
        &items(),
        &collection_holding(vec![(BRATON_PATH, 1, None)]),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].status, OrderStatus::Unverifiable);
}

/// The badge on a collection card is a join the frontend cannot make on its own: an order names the
/// market's opaque id and a card names a `/Lotus/` row, and the two namespaces share nothing. The
/// reconciliation already holds the item table, so it names the row for the interface, including
/// for orders it declines to judge, because an unverifiable order is still a live listing whose
/// holding the card should be able to speak about.
#[test]
fn a_reconciled_order_names_the_collection_row_it_belongs_to() {
    let mut ranked = sell_order(MOD_ID, 1, 1_785_405_600);
    ranked.rank = Some(5);
    let mut refinement = sell_order(RELIC_ID, 1, 1_785_405_600);
    refinement.subtype = Some("intact".to_owned());
    let orders = vec![sell_order(BRATON_ID, 2, 1_785_405_600), ranked, refinement];

    let reconciled = reconcile_orders(
        &orders,
        &items(),
        &Collection::default(),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].row_id.as_deref(), Some(BRATON_PATH));
    assert_eq!(
        reconciled[1].row_id.as_deref(),
        Some(&*format!("{MOD_PATH}#5"))
    );
    assert_eq!(
        reconciled[2].row_id.as_deref(),
        Some("/Lotus/Types/Game/Projections/T4VoidProjectionEBronze")
    );
}

/// The row an order cannot name: a set, whose path is the built item rather than the parts held,
/// and a retired item the market publishes no path for. Each carries no row, and the interface
/// offers neither badge nor edit on the strength of a row that does not exist.
#[test]
fn an_order_that_names_no_row_says_so() {
    let orders = vec![
        sell_order(SET_ID, 1, 1_785_405_600),
        sell_order(RETIRED_ID, 1, 1_785_405_600),
    ];

    let reconciled = reconcile_orders(
        &orders,
        &items(),
        &Collection::default(),
        Some(&snapshot_at(1_785_492_000)),
        None,
    );

    assert_eq!(reconciled[0].row_id, None);
    assert_eq!(reconciled[1].row_id, None);
}

const SET_PARTS_ITEMS: &str = r#"{"apiVersion":"0.25.0","data":[
    {"id":"set-braton","gameRef":"/Lotus/Weapons/Tenno/Rifle/BratonPrime",
     "i18n":{"en":{"name":"Braton Prime Set"}}},
    {"id":"part-braton-bp","gameRef":"/Lotus/Types/Recipes/Weapons/BratonPrimeBlueprint",
     "i18n":{"en":{"name":"Braton Prime Blueprint"}}},
    {"id":"part-braton-barrel","gameRef":"/Lotus/Types/Recipes/Weapons/WeaponParts/BratonPrimeBarrel",
     "i18n":{"en":{"name":"Braton Prime Barrel"}}},
    {"id":"part-braton-stock","gameRef":"/Lotus/Types/Recipes/Weapons/WeaponParts/BratonPrimeStock",
     "i18n":{"en":{"name":"Braton Prime Stock"}}},
    {"id":"set-shedu","gameRef":"/Lotus/Weapons/Sentients/Shedu/SheduHeavyWeapon",
     "i18n":{"en":{"name":"Shedu Set"}}},
    {"id":"part-shedu-barrel","gameRef":"/Lotus/Types/Recipes/Weapons/WeaponParts/SheduHeavyWeaponBarrel",
     "i18n":{"en":{"name":"Shedu Barrel"}}},
    {"id":"part-shedu-handle","gameRef":"/Lotus/Types/Recipes/Weapons/WeaponParts/SheduHeavyWeaponHandle",
     "i18n":{"en":{"name":"Shedu Handle"}}},
    {"id":"gem","gameRef":"/Lotus/Types/Items/Gems/Deimos/DeimosEidolonGemBCutItem",
     "i18n":{"en":{"name":"Cabochon Embolos"}}}
],"error":null}"#;

/// Braton Prime with its stock taken twice, so a set's count is not read as one of each; and
/// Shedu, whose blueprint the market does not sell and whose build also takes a gem the market
/// does sell, neither of which a Shedu set hands over.
const SET_PARTS_WFCD: &[u8] = br#"[
  {"uniqueName":"/Lotus/Weapons/Tenno/Rifle/BratonPrime","name":"Braton Prime","type":"Rifle","category":"Primary","masterable":true,
   "components":[
     {"uniqueName":"/Lotus/Types/Recipes/Weapons/BratonPrimeBlueprint","name":"Blueprint","tradable":true,"ducats":25},
     {"uniqueName":"/Lotus/Types/Recipes/Weapons/WeaponParts/BratonPrimeBarrel","name":"Barrel","tradable":true,"ducats":15},
     {"uniqueName":"/Lotus/Types/Recipes/Weapons/WeaponParts/BratonPrimeStock","name":"Stock","tradable":true,"ducats":25,"itemCount":2},
     {"uniqueName":"/Lotus/Types/Items/MiscItems/OrokinCell","name":"Orokin Cell","tradable":false}
   ]},
  {"uniqueName":"/Lotus/Weapons/Sentients/Shedu/SheduHeavyWeapon","name":"Shedu","type":"Rifle","category":"Primary","masterable":true,
   "components":[
     {"uniqueName":"/Lotus/Types/Recipes/Weapons/SheduHeavyWeaponBlueprint","name":"Blueprint","tradable":false},
     {"uniqueName":"/Lotus/Types/Recipes/Weapons/WeaponParts/SheduHeavyWeaponBarrel","name":"Barrel","tradable":false},
     {"uniqueName":"/Lotus/Types/Recipes/Weapons/WeaponParts/SheduHeavyWeaponHandle","name":"Handle","tradable":false},
     {"uniqueName":"/Lotus/Types/Items/Gems/Deimos/DeimosEidolonGemBCutItem","name":"Cabochon Embolos","tradable":true}
   ]},
  {"uniqueName":"/Lotus/Types/Items/Gems/Deimos/DeimosEidolonGemBCutItem","name":"Cabochon Embolos","type":"Misc","category":"Misc","masterable":false,"components":[]}
]"#;

const BRATON_BP: &str = "/Lotus/Types/Recipes/Weapons/BratonPrimeBlueprint";
const BRATON_BARREL: &str = "/Lotus/Types/Recipes/Weapons/WeaponParts/BratonPrimeBarrel";
const BRATON_STOCK: &str = "/Lotus/Types/Recipes/Weapons/WeaponParts/BratonPrimeStock";

fn judge_sets(orders: &[MarketOrder], held: Vec<(&str, u32, Option<u32>)>) -> Vec<OrderStatus> {
    let items = MarketItems::from_response(SET_PARTS_ITEMS.as_bytes()).expect("items parse");
    let recipes = CatalogIndex::from_wfcd_json(SET_PARTS_WFCD).expect("catalog parses");
    reconcile_orders(
        orders,
        &items,
        &collection_holding(held),
        Some(&snapshot_at(1_785_492_000)),
        Some(&recipes),
    )
    .into_iter()
    .map(|entry| entry.status)
    .collect()
}

/// With the recipe loaded, a set is judged by how many whole builds its parts complete, each part
/// counted at the copies one build takes.
#[test]
fn a_set_order_is_judged_by_the_sets_its_parts_complete() {
    let full = vec![
        (BRATON_BP, 2, None),
        (BRATON_BARREL, 2, None),
        (BRATON_STOCK, 4, None),
    ];
    assert_eq!(
        judge_sets(&[sell_order("set-braton", 2, 1_785_405_600)], full),
        vec![OrderStatus::Ok]
    );

    // Two of each part is two blueprints and barrels but one build's worth of stocks.
    let short = vec![
        (BRATON_BP, 2, None),
        (BRATON_BARREL, 2, None),
        (BRATON_STOCK, 2, None),
    ];
    assert_eq!(
        judge_sets(&[sell_order("set-braton", 2, 1_785_405_600)], short),
        vec![OrderStatus::Overshoot { owned: 1 }]
    );

    let none = vec![(BRATON_BP, 1, None), (BRATON_STOCK, 2, None)];
    assert_eq!(
        judge_sets(&[sell_order("set-braton", 1, 1_785_405_600)], none),
        vec![OrderStatus::Missing]
    );
}

/// A part listed on its own takes its copies first, and the set is measured against what is left,
/// so a barrel listed beside a set the same barrel completes is not counted twice.
#[test]
fn part_listings_spend_their_parts_before_a_set_does() {
    let one_build = vec![
        (BRATON_BP, 1, None),
        (BRATON_BARREL, 1, None),
        (BRATON_STOCK, 2, None),
    ];
    let orders = [
        sell_order("set-braton", 1, 1_785_405_600),
        sell_order("part-braton-barrel", 1, 1_785_405_600),
    ];

    assert_eq!(
        judge_sets(&orders, one_build),
        vec![OrderStatus::Missing, OrderStatus::Ok]
    );
}

/// Two listings of one set share one pool of parts: the second is measured against what the first
/// leaves, whatever order they arrive in.
#[test]
fn a_second_listing_of_a_set_gets_what_the_first_leaves() {
    let one_build = vec![
        (BRATON_BP, 1, None),
        (BRATON_BARREL, 1, None),
        (BRATON_STOCK, 2, None),
    ];
    let orders = [
        sell_order("set-braton", 1, 1_785_405_600),
        sell_order("set-braton", 1, 1_785_405_600),
    ];

    assert_eq!(
        judge_sets(&orders, one_build),
        vec![OrderStatus::Ok, OrderStatus::Missing]
    );
}

/// A set hands over only what the market sells as its parts: not Shedu's blueprint, which does not
/// trade, and not the gem the build also takes, which trades on its own.
#[test]
fn a_set_needs_only_the_components_the_market_sells_as_parts() {
    let parts = vec![
        (
            "/Lotus/Types/Recipes/Weapons/WeaponParts/SheduHeavyWeaponBarrel",
            1,
            None,
        ),
        (
            "/Lotus/Types/Recipes/Weapons/WeaponParts/SheduHeavyWeaponHandle",
            1,
            None,
        ),
    ];

    assert_eq!(
        judge_sets(&[sell_order("set-shedu", 1, 1_785_405_600)], parts),
        vec![OrderStatus::Ok]
    );
}

/// The snapshot rule holds for sets as for anything: a listing placed after the inventory was
/// read may be backed by parts picked up since.
#[test]
fn a_set_order_newer_than_the_snapshot_is_unverifiable() {
    assert_eq!(
        judge_sets(&[sell_order("set-braton", 1, 1_785_500_000)], Vec::new()),
        vec![OrderStatus::Unverifiable]
    );
}
