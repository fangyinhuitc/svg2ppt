//! 调试用：打印 usvg 树里各节点的绝对变换，尤其是文本展平子树。

use std::sync::Arc;

use svg2ppt::geom::decompose;

fn main() {
    let path = std::env::args().nth(1).expect("usage: inspect <file.svg>");
    let data = std::fs::read(&path).unwrap();
    let mut fontdb = usvg::fontdb::Database::new();
    fontdb.load_system_fonts();
    let opt = usvg::Options {
        fontdb: Arc::new(fontdb),
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(&data, &opt).unwrap();
    println!(
        "tree size: {:?}",
        (tree.size().width(), tree.size().height())
    );
    for child in tree.root().children() {
        walk(child, 0);
    }
}

fn walk(node: &usvg::Node, depth: usize) {
    let pad = "  ".repeat(depth);
    match node {
        usvg::Node::Group(g) => {
            println!(
                "{}Group id={:?} abs={:?}",
                pad,
                g.id(),
                decompose(g.abs_transform())
            );
            for c in g.children() {
                walk(c, depth + 1);
            }
        }
        usvg::Node::Path(p) => {
            let bbox = p.abs_bounding_box();
            println!(
                "{}Path abs={:?} bbox=({:.1},{:.1},{:.1},{:.1})",
                pad,
                decompose(p.abs_transform()),
                bbox.left(),
                bbox.top(),
                bbox.width(),
                bbox.height()
            );
        }
        usvg::Node::Image(_) => println!("{}Image", pad),
        usvg::Node::Text(t) => {
            println!("{}Text abs={:?}", pad, decompose(t.abs_transform()));
            let flat = t.flattened();
            println!(
                "{}  flattened.abs={:?} children={}",
                pad,
                decompose(flat.abs_transform()),
                flat.children().len()
            );
            for c in flat.children().iter().take(3) {
                match c {
                    usvg::Node::Group(g) => {
                        println!(
                            "{}    child Group abs={:?} bbox=({:.1},{:.1},{:.1},{:.1}) children={}",
                            pad,
                            decompose(g.abs_transform()),
                            g.abs_bounding_box().left(),
                            g.abs_bounding_box().top(),
                            g.abs_bounding_box().width(),
                            g.abs_bounding_box().height(),
                            g.children().len()
                        );
                    }
                    usvg::Node::Path(p) => {
                        let b = p.abs_bounding_box();
                        println!(
                            "{}    child Path abs={:?} bbox=({:.1},{:.1},{:.1},{:.1})",
                            pad,
                            decompose(p.abs_transform()),
                            b.left(),
                            b.top(),
                            b.width(),
                            b.height()
                        );
                    }
                    _ => println!("{}    child {:?}", pad, "other"),
                }
            }
        }
    }
}
