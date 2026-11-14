//! Library search and "add a whole folder".

use sppy_proto::{FileId, LibraryNode};

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub file_id: FileId,
    pub name: String,
    /// `Root / Season 4`
    pub folder: String,
    pub size: u64,
}

/// Files whose folder path and name contain every word of the query (case-insensitive),
/// in tree order.
pub fn search(roots: &[LibraryNode], query: &str) -> Vec<Hit> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut out = Vec::new();
    if words.is_empty() {
        return out;
    }
    fn walk(n: &LibraryNode, path: &mut Vec<String>, words: &[String], out: &mut Vec<Hit>) {
        match n {
            LibraryNode::Dir { name, children } => {
                path.push(name.clone());
                for c in children {
                    walk(c, path, words, out);
                }
                path.pop();
            }
            LibraryNode::File { file_id, name, size, .. } => {
                let folder = path.join(" / ");
                let hay = format!("{folder} / {name}").to_lowercase();
                if words.iter().all(|w| hay.contains(w.as_str())) {
                    out.push(Hit {
                        file_id: file_id.clone(),
                        name: name.clone(),
                        folder,
                        size: *size,
                    });
                }
            }
        }
    }
    for r in roots {
        walk(r, &mut Vec::new(), &words, &mut out);
    }
    out
}

/// Every file below a node, in tree order (for "add this folder").
pub fn files_under(node: &LibraryNode) -> Vec<FileId> {
    match node {
        LibraryNode::File { file_id, .. } => vec![file_id.clone()],
        LibraryNode::Dir { children, .. } => children.iter().flat_map(files_under).collect(),
    }
}

#[cfg(test)]
mod tests {
    use sppy_proto::{FileId, LibraryNode};

    use super::*;

    fn file(id: &str, name: &str) -> LibraryNode {
        LibraryNode::File {
            file_id: FileId(id.into()),
            name: name.into(),
            size: 1,
            fingerprint: None,
            subtitles: vec![],
        }
    }

    fn tree() -> Vec<LibraryNode> {
        vec![LibraryNode::Dir {
            name: "Stargate SG-1".into(),
            children: vec![
                LibraryNode::Dir {
                    name: "Season 4".into(),
                    children: vec![
                        file("a", "S04E12 - Tangent.mkv"),
                        file("b", "S04E13 - The Curse.mkv"),
                    ],
                },
                LibraryNode::Dir {
                    name: "Season 5".into(),
                    children: vec![file("c", "S05E01 - Enemies.mkv")],
                },
            ],
        }]
    }

    #[test]
    fn search_matches_all_words_in_path_and_name() {
        let hits = search(&tree(), "season 4 curse");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_id, FileId("b".into()));
        assert_eq!(hits[0].folder, "Stargate SG-1 / Season 4");
        assert_eq!(search(&tree(), "S04").len(), 2);
        assert!(search(&tree(), "nothing").is_empty());
        assert!(search(&tree(), "  ").is_empty());
    }

    #[test]
    fn files_under_a_folder_in_tree_order() {
        let LibraryNode::Dir { children, .. } = &tree()[0] else { panic!() };
        assert_eq!(
            files_under(&tree()[0]),
            vec![FileId("a".into()), FileId("b".into()), FileId("c".into())]
        );
        assert_eq!(files_under(&children[1]), vec![FileId("c".into())]);
    }
}
