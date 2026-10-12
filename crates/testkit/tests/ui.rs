//! 把 typestate 保证钉成编译失败（datascope-typestate.md ）。

#[test]
fn typestate_holds() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/query_without_scope.rs");
    t.compile_fail("tests/ui/entity_without_data_scope.rs");
}
