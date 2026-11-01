use ryu_feedback_board::model::{
    CreateGuestRequest, PublicRequest, PublicStatus, Request, RequestPatch,
};
use ryu_feedback_board::store::{Store, StoreError};

#[test]
fn duplicate_votes_do_not_inflate_demand() {
    let store = Store::open_memory().unwrap();
    let request = store
        .create_guest_request(
            "feedback",
            CreateGuestRequest {
                title: "CSV export".into(),
                body: "Export rows".into(),
                category: "Ideas".into(),
                ..CreateGuestRequest::default()
            },
            "voter-a",
        )
        .unwrap();

    assert_eq!(store.vote(&request.id, "voter-a").unwrap().vote_count, 1);
    assert_eq!(store.vote(&request.id, "voter-a").unwrap().vote_count, 1);
}

#[test]
fn public_projection_omits_private_build_context() {
    let request = Request::fixture();
    let public = PublicRequest::from_request(
        request,
        PublicStatus {
            code: "review".into(),
            label: "Under review".into(),
            tone: "amber".into(),
            terminal: false,
        },
    );
    let json = serde_json::to_value(public).unwrap();
    assert!(json.get("internal_notes").is_none());
    assert!(json.get("plan_id").is_none());
    assert!(json.get("workflow_run_id").is_none());
}

#[test]
fn stale_request_revision_is_rejected() {
    let store = Store::open_memory().unwrap();
    let request = store
        .create_guest_request(
            "feedback",
            CreateGuestRequest {
                title: "Keyboard shortcuts".into(),
                body: "Make common actions faster.".into(),
                category: "Ideas".into(),
                ..CreateGuestRequest::default()
            },
            "voter-a",
        )
        .unwrap();
    let updated = store
        .patch_request(
            &request.id,
            0,
            RequestPatch {
                status: Some("planned".into()),
                ..RequestPatch::default()
            },
        )
        .unwrap();
    assert_eq!(updated.revision, 1);
    let error = store
        .patch_request(
            &request.id,
            0,
            RequestPatch {
                status: Some("shipped".into()),
                ..RequestPatch::default()
            },
        )
        .unwrap_err();
    assert!(error.downcast_ref::<StoreError>().is_some());
}

#[test]
fn merge_moves_public_activity_to_the_survivor() {
    let store = Store::open_memory().unwrap();
    let first = store
        .create_guest_request(
            "feedback",
            CreateGuestRequest {
                title: "Export reports".into(),
                body: "Download reports as files.".into(),
                category: "Ideas".into(),
                ..CreateGuestRequest::default()
            },
            "one",
        )
        .unwrap();
    let second = store
        .create_guest_request(
            "feedback",
            CreateGuestRequest {
                title: "Download reports".into(),
                body: "Let me save a report.".into(),
                category: "Ideas".into(),
                ..CreateGuestRequest::default()
            },
            "two",
        )
        .unwrap();
    store.vote(&first.id, "voter-a").unwrap();
    store.vote(&second.id, "voter-b").unwrap();
    let merged = store.merge_requests(&first.id, &second.id).unwrap();
    assert_eq!(merged.merged_request_id, second.id);
    assert_eq!(merged.survivor.vote_count, 2);
    assert!(store
        .public_request("feedback", &second.id)
        .unwrap()
        .is_none());
}
