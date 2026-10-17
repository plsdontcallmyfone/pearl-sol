//! Cross-language vector for `schema::order_terms_hash` (storefront order job header,
//! `poster || order_terms`). The wallet extension checks its TS port against
//! `chain/state/vectors/order_terms.json`. The file is written when missing and checked
//! byte for byte otherwise (delete it to regenerate).
use state::schema::order_terms_hash;

fn hx(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn order_terms_vector() {
    // (poster seed byte, m, n, k, max_blocks, deadline_blocks)
    let cases: [(u8, u32, u32, u32, u32, u64); 3] =
        [(7, 64, 64, 1024, 10, 500), (1, 8, 16, 1024, 1, 1), (0xab, 4096, 2048, 65536, 100000, u64::MAX)];
    let mut rows = Vec::new();
    for (s, m, n, k, mb, d) in cases {
        let poster = [s; 32];
        let h = order_terms_hash(&poster, m, n, k, mb, d);
        rows.push(format!(
            "    {{\"poster_hex\": \"{}\", \"m\": {m}, \"n\": {n}, \"k\": {k}, \"max_blocks\": {mb}, \"deadline_blocks\": \"{d}\", \"order_terms\": \"{}\"}}",
            hx(&poster),
            hx(&h)
        ));
    }
    let json = format!(
        "{{\n  \"rule\": \"blake3(\\\"pearl-sol:order-terms:v1\\\" || poster || m u32 LE || n u32 LE || k u32 LE || max_blocks u32 LE || deadline_blocks u64 LE)\",\n  \"cases\": [\n{}\n  ]\n}}\n",
        rows.join(",\n")
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors/order_terms.json");
    match std::fs::read_to_string(&path) {
        Ok(existing) => assert_eq!(existing, json, "order_terms vector changed; delete {} to regenerate", path.display()),
        Err(_) => {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &json).unwrap();
            println!("wrote {}", path.display());
        }
    }
}
