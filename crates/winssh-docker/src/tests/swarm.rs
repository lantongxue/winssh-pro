use super::*;

#[test]
fn task_state_tone_groups_scheduling_states() {
    assert_eq!(task_state_tone("running"), "running");
    assert_eq!(task_state_tone("starting"), "running");
    assert_eq!(task_state_tone("pending"), "pending");
    assert_eq!(task_state_tone("assigned"), "pending");
    assert_eq!(task_state_tone("failed"), "failed");
    assert_eq!(task_state_tone("rejected"), "failed");
    assert_eq!(task_state_tone("shutdown"), "other");
}

#[test]
fn node_state_tone_ready_green_down_red() {
    assert_eq!(node_state_tone("ready"), "ready");
    assert_eq!(node_state_tone("down"), "down");
    assert_eq!(node_state_tone("disconnected"), "down");
    assert_eq!(node_state_tone("unknown"), "other");
}

#[test]
fn node_availability_round_trips() {
    for a in [
        NodeAvailability::Active,
        NodeAvailability::Pause,
        NodeAvailability::Drain,
    ] {
        assert_eq!(NodeAvailability::parse(a.as_str()), a);
    }
    assert_eq!(NodeAvailability::parse("ACTIVE"), NodeAvailability::Active);
    assert_eq!(NodeAvailability::parse("weird"), NodeAvailability::Active);
}
