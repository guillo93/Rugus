//! Tests host del log circular de faults (`rugus_fs::faultlog`) sobre el backend
//! RAM-NOR (`mock`). Verifica anillo FIFO acotado, persistencia del contador y
//! supervivencia al remontaje. Sólo API pública.

use rugus_fs::faultlog::{FaultLog, FaultRecord};
use rugus_fs::mock::RamFlash;
use rugus_fs::Rufs;

const SEC: usize = 4096;
const PG: usize = 256;

fn fresh() -> Rufs<RamFlash, 32> {
    Rufs::mount(RamFlash::new(8, SEC, PG)).unwrap()
}

#[test]
fn empty_log() {
    let mut fs = fresh();
    let log = FaultLog::<4>::open(&mut fs).unwrap();
    assert_eq!(log.total(), 0);
    assert_eq!(log.stored(), 0);
    let mut seen = 0;
    log.for_each(&mut fs, |_| seen += 1).unwrap();
    assert_eq!(seen, 0);
}

#[test]
fn record_and_iterate_in_order() {
    let mut fs = fresh();
    let mut log = FaultLog::<8>::open(&mut fs).unwrap();
    for i in 0..3u32 {
        let s = log.record(&mut fs, 0x1000 + i, i * 7).unwrap();
        assert_eq!(s, i);
    }
    assert_eq!(log.total(), 3);
    assert_eq!(log.stored(), 3);
    let mut out: Vec<FaultRecord> = Vec::new();
    log.for_each(&mut fs, |r| out.push(r)).unwrap();
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].seq, 0);
    assert_eq!(out[0].kind, 0x1000);
    assert_eq!(out[2].seq, 2);
    assert_eq!(out[2].arg, 14);
}

#[test]
fn ring_wraps_keeping_newest() {
    let mut fs = fresh();
    let mut log = FaultLog::<4>::open(&mut fs).unwrap();
    // 10 faults en un anillo de 4: deben quedar los seq 6..=9.
    for i in 0..10u32 {
        log.record(&mut fs, i, i).unwrap();
    }
    assert_eq!(log.total(), 10);
    assert_eq!(log.stored(), 4);
    let mut out: Vec<FaultRecord> = Vec::new();
    log.for_each(&mut fs, |r| out.push(r)).unwrap();
    assert_eq!(out.len(), 4);
    let seqs: Vec<u32> = out.iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![6, 7, 8, 9]);
}

#[test]
fn survives_remount() {
    let mut fs = Rufs::<_, 32>::mount(RamFlash::new(8, SEC, PG)).unwrap();
    {
        let mut log = FaultLog::<4>::open(&mut fs).unwrap();
        log.record(&mut fs, 0xDEAD, 0xBEEF).unwrap();
        log.record(&mut fs, 0xCAFE, 0xF00D).unwrap();
    }
    let dev = fs.into_device();
    let mut fs2 = Rufs::<_, 32>::mount(dev).unwrap();
    let log2 = FaultLog::<4>::open(&mut fs2).unwrap();
    assert_eq!(log2.total(), 2);
    assert_eq!(log2.stored(), 2);
    let mut out: Vec<FaultRecord> = Vec::new();
    log2.for_each(&mut fs2, |r| out.push(r)).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].kind, 0xDEAD);
    assert_eq!(out[1].kind, 0xCAFE);
    // Continúa la secuencia tras remontar.
    let mut log2 = log2;
    let s = log2.record(&mut fs2, 1, 1).unwrap();
    assert_eq!(s, 2);
}
