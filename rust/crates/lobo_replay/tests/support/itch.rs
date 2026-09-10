use lobo_primitives::uuid::Uuid;

pub struct File(pub std::path::PathBuf);
impl Drop for File {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn record(tag: u8, locate: u16, time: u64, body: &[u8]) -> Vec<u8> {
    let mut bytes = ((11 + body.len()) as u16).to_be_bytes().to_vec();
    bytes.push(tag);
    bytes.extend(locate.to_be_bytes());
    bytes.extend(0u16.to_be_bytes());
    bytes.extend(&time.to_be_bytes()[2..]);
    bytes.extend(body);
    bytes
}
fn directory(locate: u16, symbol: &[u8; 8]) -> Vec<u8> {
    let mut body = symbol.to_vec();
    body.extend(b"QN");
    body.extend(100u32.to_be_bytes());
    body.extend(b"NCC PNN1N");
    body.extend(1u32.to_be_bytes());
    body.push(b'N');
    assert_eq!(body.len(), 28);
    record(b'R', locate, 0, &body)
}
fn add(locate: u16, id: u64, qty: u32, price: u32, symbol: &[u8; 8]) -> Vec<u8> {
    let mut body = id.to_be_bytes().to_vec();
    body.push(b'B');
    body.extend(qty.to_be_bytes());
    body.extend(symbol);
    body.extend(price.to_be_bytes());
    record(b'A', locate, id, &body)
}
pub fn file() -> File {
    let mut execution = 1u64.to_be_bytes().to_vec();
    execution.extend(10u32.to_be_bytes());
    execution.extend(1u64.to_be_bytes());
    let mut priced = execution.clone();
    priced.push(b'Y');
    priced.extend(10200u32.to_be_bytes());
    let mut cancel = 1u64.to_be_bytes().to_vec();
    cancel.extend(10u32.to_be_bytes());
    let mut replace = 1u64.to_be_bytes().to_vec();
    replace.extend(3u64.to_be_bytes());
    replace.extend(70u32.to_be_bytes());
    replace.extend(10400u32.to_be_bytes());
    let bytes = [
        directory(1, b"ALPHA   "),
        directory(2, b"BETA    "),
        record(b'S', 0, 0, b"S"),
        add(1, 1, 100, 10000, b"ALPHA   "),
        add(2, 2, 20, 20000, b"BETA    "),
        record(b'E', 1, 3, &execution),
        record(b'C', 1, 4, &priced),
        record(b'X', 1, 5, &cancel),
        record(b'U', 1, 6, &replace),
        record(b'D', 1, 7, &3u64.to_be_bytes()),
        add(1, 4, 40, 11000, b"ALPHA   "),
    ]
    .concat();
    let file =
        File(std::env::temp_dir().join(format!("lobo-custom-source-{}.itch", Uuid::new_v4())));
    std::fs::write(&file.0, bytes).unwrap();
    file
}
