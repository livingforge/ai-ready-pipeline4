use super::*;

fn packet(count: usize, length: usize) -> Value {
    json!({"packet":"fixture","document":"doc","sources":{"tables":[{"sheet":"A"},{"sheet":"B"}],
        "rows":(0..count).map(|i|json!([format!("s{}",i+1),i%2,"A1","value","語".repeat(length)])).collect::<Vec<_>>()}})
}

#[test]
fn short_cells_stay_in_one_document_and_large_payloads_split_without_loss() {
    let small = packet(178, 5);
    assert!(
        partitions(&small, &[], 0, 0, 98304, &json!({}))
            .unwrap()
            .is_empty()
    );
    let large = packet(180, 400);
    let regions = partitions(&large, &[], 0, 0, 98304, &json!({})).unwrap();
    assert!(regions.len() > 2);
    let mut seen = BTreeSet::new();
    for region in &regions {
        let scoped = scoped_packet(&large, region, &json!({})).unwrap();
        assert!(task_text(&scoped).len() <= 98304);
        for source in region["sources"].as_array().unwrap() {
            assert!(seen.insert(source.as_str().unwrap()));
        }
    }
    assert_eq!(seen.len(), 180);
    assert!(
        !partitions(&small, &[], 0, 120, 98304, &json!({}))
            .unwrap()
            .is_empty()
    );
    assert!(partitions(&packet(1, 50000), &[], 0, 0, 98304, &json!({})).is_err());
}
