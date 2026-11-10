use super::*;
use crate::{CreatePurse, PurseId, PurseTag, ReadPurse};

#[test]
fn duplicate_registration_cannot_replace_a_handler() {
    let mut registry = Registry::new();
    registry.register::<ReadPurse>().unwrap();
    assert_eq!(
        registry.register::<ReadPurse>(),
        Err(Error::Protocol("duplicate operation: nft.purse.v1".into()))
    );
}

#[test]
fn invalid_requests_leave_the_database_unchanged() {
    let registry = registry().unwrap();
    let mut db = Db::memory().unwrap();
    let request = postcard::to_allocvec(&CreatePurse {
        name: "must not be inserted".into(),
        tag: PurseTag(vec![1]),
    })
    .unwrap();
    assert!(matches!(
        registry.dispatch("missing.v1", &request, &mut db),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        registry.dispatch(CreatePurse::NAME, &[], &mut db),
        Err(Error::Protocol(_))
    ));
    let mut trailing = request;
    trailing.push(0);
    assert!(matches!(
        registry.dispatch(CreatePurse::NAME, &trailing, &mut db),
        Err(Error::Protocol(_))
    ));
    assert_eq!(ReadPurse { id: PurseId(0) }.execute(&mut db).unwrap(), None);
}

#[test]
fn dispatch_uses_the_registered_output_type() {
    let registry = registry().unwrap();
    let mut db = Db::memory().unwrap();
    let request = postcard::to_allocvec(&CreatePurse {
        name: "Savings".into(),
        tag: PurseTag(vec![255, 0]),
    })
    .unwrap();
    let response = registry
        .dispatch(CreatePurse::NAME, &request, &mut db)
        .unwrap();
    assert_eq!(
        postcard::from_bytes::<Option<PurseId>>(&response).unwrap(),
        Some(PurseId(0))
    );
}
