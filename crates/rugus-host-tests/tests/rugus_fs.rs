//! Tests host de `rugus-fs` (almacén clave-valor log-structured) contra el
//! backend RAM que emula la semántica NOR (`program` 1->0, `erase` a 0xFF) e
//! inyecta cortes de energía. Sólo API pública.

use rugus_fs::crc::crc32;
use rugus_fs::mock::RamFlash;
use rugus_fs::{Error, Rufs};

const SEC: usize = 4096;
const PG: usize = 256;

fn fresh(sectors: usize) -> Rufs<RamFlash, 32> {
    Rufs::mount(RamFlash::new(sectors, SEC, PG)).unwrap()
}

#[test]
fn crc_known_vectors() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

#[test]
fn set_get_basic() {
    let mut fs = fresh(8);
    fs.set(b"hostname", b"rugus-f769").unwrap();
    let mut buf = [0u8; 64];
    let n = fs.get(b"hostname", &mut buf).unwrap();
    assert_eq!(&buf[..n], b"rugus-f769");
    assert!(fs.contains(b"hostname"));
    assert_eq!(fs.len(), 1);
}

#[test]
fn overwrite_latest_wins() {
    let mut fs = fresh(8);
    fs.set(b"k", b"v1").unwrap();
    fs.set(b"k", b"v2-longer").unwrap();
    let mut buf = [0u8; 64];
    let n = fs.get(b"k", &mut buf).unwrap();
    assert_eq!(&buf[..n], b"v2-longer");
    assert_eq!(fs.len(), 1);
}

#[test]
fn delete_then_missing() {
    let mut fs = fresh(8);
    fs.set(b"k", b"v").unwrap();
    fs.delete(b"k").unwrap();
    let mut buf = [0u8; 8];
    assert_eq!(fs.get(b"k", &mut buf), Err(Error::NotFound));
    assert!(!fs.contains(b"k"));
    assert_eq!(fs.len(), 0);
}

#[test]
fn remount_persists() {
    let mut fs = Rufs::<_, 32>::mount(RamFlash::new(8, SEC, PG)).unwrap();
    fs.set(b"a", b"1").unwrap();
    fs.set(b"b", b"2").unwrap();
    fs.set(b"a", b"3").unwrap();
    fs.delete(b"b").unwrap();
    let dev = fs.into_device();
    let mut fs2 = Rufs::<_, 32>::mount(dev).unwrap();
    let mut buf = [0u8; 16];
    let n = fs2.get(b"a", &mut buf).unwrap();
    assert_eq!(&buf[..n], b"3");
    assert_eq!(fs2.get(b"b", &mut buf), Err(Error::NotFound));
    assert_eq!(fs2.len(), 1);
}

#[test]
fn power_fail_mid_write_is_ignored() {
    // 1) FS sana con un registro comprometido.
    let mut fs = Rufs::<_, 32>::mount(RamFlash::new(8, SEC, PG)).unwrap();
    fs.set(b"good", b"value").unwrap();
    let mut dev = fs.into_device();

    // 2) Simula corte: el siguiente `program` se trunca a 100 bytes.
    dev.set_power_fail_after(100);
    let mut fs = Rufs::<_, 32>::mount(dev).unwrap();
    let r = fs.set(b"torn", &[0xAB; 200]); // debe fallar a mitad
    assert!(r.is_err());
    let dev = fs.into_device();

    // 3) Remontar: "good" intacto, "torn" descartado por CRC.
    let mut fs2 = Rufs::<_, 32>::mount(dev).unwrap();
    let mut buf = [0u8; 16];
    let n = fs2.get(b"good", &mut buf).unwrap();
    assert_eq!(&buf[..n], b"value");
    assert_eq!(fs2.get(b"torn", &mut buf), Err(Error::NotFound));
}

#[test]
fn wrap_and_compaction() {
    let mut fs = fresh(4);
    for i in 0..500u32 {
        fs.set(b"counter", &i.to_le_bytes()).unwrap();
    }
    let mut buf = [0u8; 4];
    let n = fs.get(b"counter", &mut buf).unwrap();
    assert_eq!(u32::from_le_bytes(buf[..n].try_into().unwrap()), 499);
    assert_eq!(fs.len(), 1);
    let dev = fs.into_device();
    let mut fs2 = Rufs::<_, 32>::mount(dev).unwrap();
    let n = fs2.get(b"counter", &mut buf).unwrap();
    assert_eq!(u32::from_le_bytes(buf[..n].try_into().unwrap()), 499);
}

#[test]
fn many_keys_survive_compaction() {
    let mut fs = fresh(6);
    for k in 0..20u8 {
        fs.set(&[b'k', k], &[k; 100]).unwrap();
    }
    for _ in 0..30 {
        for k in 0..20u8 {
            fs.set(&[b'k', k], &[k.wrapping_add(1); 120]).unwrap();
        }
    }
    let dev = fs.into_device();
    let mut fs2 = Rufs::<_, 32>::mount(dev).unwrap();
    assert_eq!(fs2.len(), 20);
    for k in 0..20u8 {
        let mut buf = [0u8; 128];
        let n = fs2.get(&[b'k', k], &mut buf).unwrap();
        assert!(buf[..n].iter().all(|&b| b == k.wrapping_add(1)));
    }
}
