use flate2::read::GzDecoder;
use shared::types::Reference;
use shared::{
    cell_key_from_i16, quantize, write_index, CellEntry, TreeNode, TreeRootEntry, EMPTY_NODE,
    TREE_LEAF_SIZE,
};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

const SPLIT_DIMS: [usize; 11] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 13];
const MAX_INDEX_BYTES: u64 = 135 * 1024 * 1024;

#[derive(Clone)]
struct Entry {
    vector: [i16; 16],
    label: u8,
    cell: u16,
    original_id: u32,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut refs_path = PathBuf::new();
    let mut out_path = PathBuf::new();
    let mut leaf_size = TREE_LEAF_SIZE;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--refs" => {
                refs_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--norm" => {
                // Constants are embedded in the shared crate; accepted for CLI compatibility.
                i += 2;
            }
            "--mcc" => {
                // Constants are embedded in the shared crate; accepted for CLI compatibility.
                i += 2;
            }
            "--out" => {
                out_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--leaf-size" => {
                leaf_size = args[i + 1].parse::<usize>().expect("invalid leaf size");
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    assert!(leaf_size > 0 && leaf_size <= 512, "leaf size out of range");

    eprintln!("Loading references from {:?}...", refs_path);
    let f = File::open(&refs_path).expect("failed to open refs file");
    let gz = GzDecoder::new(f);
    let reader = BufReader::new(gz);
    let refs: Vec<Reference> = serde_json::from_reader(reader).expect("failed to parse refs");
    let n = refs.len();
    eprintln!("Loaded {} references", n);

    eprintln!("Quantizing and computing cell keys...");
    let mut entries: Vec<Entry> = refs
        .iter()
        .enumerate()
        .map(|(original_id, r)| {
            let v14: [f64; 14] = r.vector[..14].try_into().expect("bad vector length");
            let vector = quantize(&v14);
            let cell = cell_key_from_i16(&vector);
            let label = if r.label == "fraud" { 1u8 } else { 0u8 };
            Entry {
                vector,
                label,
                cell,
                original_id: original_id as u32,
            }
        })
        .collect();

    // Keep each cell contiguous so SEARCH_ENGINE=cell remains a comparable baseline.
    entries.sort_by_key(|e| (e.cell, e.original_id));
    eprintln!("Sorted by cell key");

    let mut cell_dir = [CellEntry::default(); 8192];
    let mut ranges = Vec::new();
    let mut n_cells_populated = 0u32;
    {
        let mut pos = 0usize;
        while pos < n {
            let cell = entries[pos].cell as usize;
            let start = pos;
            while pos < n && entries[pos].cell as usize == cell {
                pos += 1;
            }
            cell_dir[cell] = CellEntry {
                offset: start as u32,
                count: (pos - start) as u32,
            };
            ranges.push((cell as u16, start, pos));
            n_cells_populated += 1;
        }
    }
    eprintln!(
        "Cell directory built. Populated cells: {}",
        n_cells_populated
    );

    eprintln!("Building exact tree nodes with leaf_size={}...", leaf_size);
    let mut tree_roots = [TreeRootEntry {
        root: EMPTY_NODE,
        count: 0,
    }; 8192];
    let mut populated_tree_keys = Vec::with_capacity(ranges.len());
    let mut tree_nodes = Vec::with_capacity((n / leaf_size) * 2);

    for (cell, start, end) in ranges {
        let root = build_tree(&mut entries, start, end, &mut tree_nodes, leaf_size);
        tree_roots[cell as usize] = TreeRootEntry {
            root,
            count: (end - start) as u32,
        };
        populated_tree_keys.push(cell as u32);
    }

    let vectors: Vec<[i16; 16]> = entries.iter().map(|e| e.vector).collect();
    let labels: Vec<u8> = entries.iter().map(|e| e.label).collect();
    let original_ids: Vec<u32> = entries.iter().map(|e| e.original_id).collect();

    eprintln!(
        "Writing index to {:?} with {} tree nodes...",
        out_path,
        tree_nodes.len()
    );
    write_index(
        &out_path,
        &vectors,
        &labels,
        &original_ids,
        &cell_dir,
        &tree_roots,
        &populated_tree_keys,
        &tree_nodes,
        n_cells_populated,
        leaf_size as u32,
    )
    .expect("failed to write index");

    let size = std::fs::metadata(&out_path).unwrap().len();
    eprintln!("Done. Index written ({} MB)", size / 1_048_576);
    assert!(
        size <= MAX_INDEX_BYTES,
        "index.bin is {} bytes, above {} byte budget",
        size,
        MAX_INDEX_BYTES
    );
}

fn build_tree(
    entries: &mut [Entry],
    start: usize,
    end: usize,
    nodes: &mut Vec<TreeNode>,
    leaf_size: usize,
) -> u32 {
    let node_idx = nodes.len() as u32;
    nodes.push(TreeNode::default());

    let (bbox_min, bbox_max, min_original_id) = summarize(&entries[start..end]);
    let count = end - start;

    if count <= leaf_size {
        nodes[node_idx as usize] = TreeNode {
            bbox_min,
            bbox_max,
            left: EMPTY_NODE,
            right: EMPTY_NODE,
            offset: start as u32,
            count: count as u32,
            min_original_id,
            flags: TreeNode::LEAF_FLAG,
        };
        return node_idx;
    }

    let split_dim = choose_split_dim(&bbox_min, &bbox_max);
    let mid = start + count / 2;
    entries[start..end].select_nth_unstable_by(mid - start, |a, b| {
        a.vector[split_dim]
            .cmp(&b.vector[split_dim])
            .then_with(|| a.original_id.cmp(&b.original_id))
    });

    let left = build_tree(entries, start, mid, nodes, leaf_size);
    let right = build_tree(entries, mid, end, nodes, leaf_size);

    nodes[node_idx as usize] = TreeNode {
        bbox_min,
        bbox_max,
        left,
        right,
        offset: 0,
        count: count as u32,
        min_original_id,
        flags: 0,
    };
    node_idx
}

fn summarize(entries: &[Entry]) -> ([i16; 16], [i16; 16], u32) {
    let mut bbox_min = [i16::MAX; 16];
    let mut bbox_max = [i16::MIN; 16];
    let mut min_original_id = u32::MAX;

    for entry in entries {
        min_original_id = min_original_id.min(entry.original_id);
        for dim in 0..16 {
            bbox_min[dim] = bbox_min[dim].min(entry.vector[dim]);
            bbox_max[dim] = bbox_max[dim].max(entry.vector[dim]);
        }
    }

    (bbox_min, bbox_max, min_original_id)
}

fn choose_split_dim(bbox_min: &[i16; 16], bbox_max: &[i16; 16]) -> usize {
    let mut best_dim = SPLIT_DIMS[0];
    let mut best_range = i32::MIN;

    for &dim in &SPLIT_DIMS {
        let range = bbox_max[dim] as i32 - bbox_min[dim] as i32;
        if range > best_range || (range == best_range && dim < best_dim) {
            best_dim = dim;
            best_range = range;
        }
    }

    best_dim
}
