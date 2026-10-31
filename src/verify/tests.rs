use super::{attempt_identity, Candidate, RemovalPolicy};
use std::collections::BTreeMap;

fn minimize(mut items: Vec<char>, succeeds: impl Fn(&[char]) -> bool) -> Vec<char> {
    let mut policy = RemovalPolicy::new(items.len());
    while let Some(range) = policy.next() {
        let mut trial = items.clone();
        trial.drain(range);
        let success = succeeds(&trial);
        if success {
            items = trial;
        }
        policy.record(success, items.len());
    }
    items
}

#[test]
fn group_removal_finds_repair_hidden_from_single_removals() {
    let result = minimize(vec!['a', 'b', 'c'], |items| items == ['c'] || items == ['a', 'b', 'c']);
    assert_eq!(result, ['c']);
}

#[test]
fn group_removal_keeps_all_individually_required_repairs() {
    let result =
        minimize(vec!['a', 'b', 'c', 'd'], |items| items.contains(&'a') && items.contains(&'c'));
    assert_eq!(result, ['a', 'c']);
}

#[test]
fn candidate_attempt_changes_with_graph_and_patch_contents() {
    let mut candidate = Candidate {
        key: "patch:p".into(),
        configuration: "same".into(),
        artifacts: BTreeMap::from([("patches/p.patch".into(), "first".into())]),
    };
    let first = attempt_identity(&candidate, &[]);
    candidate.artifacts.insert("patches/p.patch".into(), "second".into());
    assert_ne!(first, attempt_identity(&candidate, &[]));
    let graph = Candidate {
        key: "dependency".into(),
        configuration: "version".into(),
        artifacts: BTreeMap::new(),
    };
    assert_ne!(attempt_identity(&candidate, &[]), attempt_identity(&candidate, &[graph]));
}
