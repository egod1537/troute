#[test]
fn tree_has_an_even_number_of_odd_degree_vertices() {
    let edges = vec![
        MstEdge {
            from: 0,
            to: 1,
            distance: 1,
        },
        MstEdge {
            from: 0,
            to: 2,
            distance: 1,
        },
        MstEdge {
            from: 0,
            to: 3,
            distance: 1,
        },
        MstEdge {
            from: 3,
            to: 4,
            distance: 1,
        },
    ];
    let odd = odd_degree_vertices(5, &edges).unwrap();
    assert_eq!(odd.len() % 2, 0);
    assert_eq!(odd, vec![0, 1, 2, 4]);
}

#[test]
fn bit_dp_finds_the_exact_minimum_matching_and_covers_each_vertex_once() {
    let vertices = vec![10, 11, 12, 13];
    let distances = vec![
        vec![0, 1, 2, 2],
        vec![1, 0, 100, 2],
        vec![2, 100, 0, 100],
        vec![2, 2, 100, 0],
    ];
    let matching = BitDpPerfectMatching::default()
        .minimum_weight_perfect_matching(&vertices, &distances)
        .unwrap();

    // Greedily taking 10-11 would cost 101 in total; exact DP finds 4.
    assert_eq!(matching.iter().map(|edge| edge.distance).sum::<u64>(), 4);
    let mut covered: Vec<_> = matching
        .iter()
        .flat_map(|edge| [edge.left, edge.right])
        .collect();
    covered.sort_unstable();
    assert_eq!(covered, vertices);
}

#[test]
fn bit_dp_rejects_an_odd_set_above_its_limit() {
    let vertices: Vec<_> = (0..22).collect();
    let distances = vec![vec![1; 22]; 22];
    assert!(matches!(
        BitDpPerfectMatching::default().minimum_weight_perfect_matching(&vertices, &distances),
        Err(SolverError::UnsupportedMatchingVertexCount {
            maximum: MAX_BIT_DP_MATCHING_VERTICES,
            actual: 22
        })
    ));
}

#[test]
fn blossom_and_bit_dp_have_the_same_small_instance_cost() {
    let vertices: Vec<_> = (0..12).collect();
    let distances = matching_distances(vertices.len());
    let bit_dp = BitDpPerfectMatching::default()
        .minimum_weight_perfect_matching(&vertices, &distances)
        .unwrap();
    let blossom = BlossomPerfectMatching
        .minimum_weight_perfect_matching(&vertices, &distances)
        .unwrap();

    assert_eq!(matching_cost(&bit_dp), matching_cost(&blossom));
}

#[test]
fn blossom_handles_a_large_odd_set_completely_and_deterministically() {
    let vertices: Vec<_> = (0..64).collect();
    let distances = matching_distances(vertices.len());
    let solve = || {
        BlossomPerfectMatching
            .minimum_weight_perfect_matching(&vertices, &distances)
            .unwrap()
    };
    let first = solve();
    let second = solve();

    assert_eq!(first, second);
    assert_eq!(first.len(), vertices.len() / 2);
    let mut covered: Vec<_> = first
        .iter()
        .flat_map(|edge| [edge.left, edge.right])
        .collect();
    covered.sort_unstable();
    assert_eq!(covered, vertices);
}

#[test]
fn auto_and_forced_matching_selection_obey_configuration() {
    let default = AutoPerfectMatching::default();
    assert_eq!(default.selected_strategy(20), MatchingStrategyChoice::BitDp);
    assert_eq!(
        default.selected_strategy(22),
        MatchingStrategyChoice::Blossom
    );

    let forced_blossom = AutoPerfectMatching::new(
        MatchingStrategyConfig::new(MatchingStrategyChoice::Blossom, 20).unwrap(),
    )
    .unwrap();
    assert_eq!(
        forced_blossom.selected_strategy(4),
        MatchingStrategyChoice::Blossom
    );
    let forced_bit_dp = AutoPerfectMatching::new(
        MatchingStrategyConfig::new(MatchingStrategyChoice::BitDp, 2).unwrap(),
    )
    .unwrap();
    assert_eq!(
        forced_bit_dp.selected_strategy(20),
        MatchingStrategyChoice::BitDp
    );
    assert_eq!(
        "auto".parse::<MatchingStrategyChoice>().unwrap(),
        MatchingStrategyChoice::Auto
    );
    assert_eq!(
        "bitdp".parse::<MatchingStrategyChoice>().unwrap(),
        MatchingStrategyChoice::BitDp
    );
    assert_eq!(
        "blossom".parse::<MatchingStrategyChoice>().unwrap(),
        MatchingStrategyChoice::Blossom
    );
}
