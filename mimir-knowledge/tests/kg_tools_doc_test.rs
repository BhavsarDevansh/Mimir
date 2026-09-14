//! Documentation contract tests for the knowledge-graph LLM tools.

use std::fs;
use std::path::Path;

#[test]
fn kg_tools_docs_name_the_current_relationship_type_lookup() {
    let docs =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/kg-tools.md"))
            .expect("kg-tools documentation should be readable");

    assert!(
        docs.contains("KnowledgeGraph::get_relationship_type_id"),
        "kg-tool documentation should name the current read-only lookup"
    );
    assert!(
        !docs.contains("get_predicate_id"),
        "kg-tool documentation should not name the removed predicate lookup"
    );
    assert!(
        !docs.contains("ensure_predicate"),
        "kg-tool documentation should not name the removed predicate mutator"
    );
}
