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
    let request = CreatePurse {
        name: "must not be inserted".into(),
        tag: PurseTag(vec![1]),
    }
    .encode();
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
fn dispatch_accepts_scale_input_and_encodes_the_typed_output() {
    let registry = registry().unwrap();
    let mut db = Db::memory().unwrap();
    let request = [28, b'S', b'a', b'v', b'i', b'n', b'g', b's', 8, 255, 0];
    let response = registry
        .dispatch(CreatePurse::NAME, &request, &mut db)
        .unwrap();
    assert_eq!(response, vec![1, 0, 0, 0, 0]);
}
