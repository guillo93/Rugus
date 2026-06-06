//! `rugus-fs` — almacén clave-valor *log-structured* resistente a corte de
//! energía sobre [`rugus_hal::BlockDevice`] (F5.C.2).
//!
//! Pensado para NOR flash (QSPI MX25L51245G de la F769I-DISCO): escribe en
//! **append puro**, nunca reescribe una página ya programada, y cada registro
//! va protegido por CRC-32. Una escritura interrumpida por pérdida de energía
//! deja como mucho un registro *roto* al final del log que el montaje descarta
//! por CRC; los datos previamente comprometidos siempre sobreviven. Es el
//! mismo principio que `littlefs`, implementado en Rust puro y `no_std` para no
//! arrastrar C/bindgen ni romper la compilación de los dos targets del CI.
//!
//! # Modelo
//!
//! El medio se ve como un anillo de **sectores** ([`BlockDevice::erase_size`]).
//! Los registros se concatenan dentro de un sector alineados a página
//! ([`BlockDevice::prog_size`]) y **nunca cruzan** un límite de sector (para que
//! borrar un sector no parta un registro). Cada registro lleva:
//!
//! ```text
//! offset  campo      tamaño  descripción
//! 0       magic      u32     0x5246_5331 ("RFS1")
//! 4       seq        u32     secuencia monotónica global
//! 8       key_len    u16     longitud de la clave (<= MAX_KEY)
//! 10      val_len    u16     longitud del valor   (<= MAX_VALUE)
//! 12      flags      u16     bit0 = tombstone (borrado)
//! 14      _rsvd      u16
//! 16      hdr_crc    u32     crc32(bytes 0..16)
//! 20      data_crc   u32     crc32(key || value)
//! 24      key || value       payload
//! ```
//!
//! El montaje escanea todos los sectores, valida CRCs y reconstruye un índice
//! en RAM `clave -> registro más reciente` (gana el `seq` mayor). Mantiene
//! siempre **un sector libre pre-borrado** de reserva; al llenarse el sector
//! activo, rota a la reserva y elige otra. Cuando no quedan sectores libres,
//! una pasada de **compactación** copia los registros vivos de un sector hacia
//! delante y lo borra (GC por copia).
//!
//! # Límites
//!
//! - `MAX_KEY` = 32 B, `MAX_VALUE` = 512 B (un registro cabe en un sector NOR
//!   de 4 KiB con holgura).
//! - `N` (parámetro const) = número máximo de claves vivas + tombstones.
//! - Requiere `sector_count >= 3`.
//! - Los *tombstones* se conservan (y se recompactan) para no resucitar claves;
//!   con un juego de claves estable su acumulación es despreciable.

#![cfg_attr(not(test), no_std)]
#![deny(missing_docs)]

pub mod crc;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

use rugus_hal::BlockDevice;

/// Longitud máxima de clave en bytes.
pub const MAX_KEY: usize = 32;
/// Longitud máxima de valor en bytes.
pub const MAX_VALUE: usize = 512;

const MAGIC: u32 = 0x5246_5331;
const HEADER_SIZE: usize = 24;
const FLAG_DELETE: u16 = 0x0001;
/// Buffer de pila para serializar/leer un registro completo (cabecera + payload
/// redondeado a página). Cota: `HEADER + MAX_KEY + MAX_VALUE` redondeado a la
/// página NOR mayor que soportamos (256 B) -> 768; redondeamos a 1024.
const SCRATCH: usize = 1024;

/// Error de la FS, genérico sobre el error del backend de bloque `E`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// Error propagado del [`BlockDevice`] subyacente.
    Device(E),
    /// Geometría del medio no soportada (sectores < 3, página < cabecera, …).
    BadGeometry,
    /// Clave o valor exceden `MAX_KEY` / `MAX_VALUE`.
    TooLarge,
    /// Se superó la capacidad del índice (`N` claves+tombstones).
    IndexFull,
    /// No queda espacio libre ni tras compactar.
    NoSpace,
    /// La clave no existe.
    NotFound,
    /// El buffer del llamante es demasiado pequeño para el valor.
    BufferTooSmall,
}

/// Entrada del índice en RAM: una clave y la localización de su registro vivo.
#[derive(Clone)]
struct Entry {
    key: heapless::Vec<u8, MAX_KEY>,
    seq: u32,
    addr: u32,
    val_len: u16,
    deleted: bool,
}

/// Almacén clave-valor sobre un [`BlockDevice`]. `N` = nº máximo de entradas de
/// índice (claves vivas + tombstones).
pub struct Rufs<D: BlockDevice, const N: usize> {
    dev: D,
    index: heapless::Vec<Entry, N>,
    next_seq: u32,
    /// Puntero de escritura absoluto (byte).
    head: u32,
    /// Sector que `head` ocupa legítimamente: su cola `[head, fin)` está borrada
    /// (`0xFF`). Si `head` deriva a otro sector (p. ej. un registro terminó justo
    /// en la frontera), `head / esz != active_sec` fuerza una rotación.
    active_sec: u32,
    /// Sector de reserva, pre-borrado, listo para rotar.
    free_sector: u32,
    sector_count: u32,
    esz: u32,
    psz: u32,
}

#[inline]
fn round_up(x: usize, m: usize) -> usize {
    x.div_ceil(m) * m
}

impl<D: BlockDevice, const N: usize> Rufs<D, N> {
    /// Monta la FS: escanea el medio, reconstruye el índice y deja una reserva
    /// libre. Idempotente frente a registros rotos por corte de energía.
    pub fn mount(dev: D) -> Result<Self, Error<D::Error>> {
        let cap = dev.capacity();
        let esz = dev.erase_size();
        let psz = dev.prog_size();
        if esz == 0 || psz == 0 || psz < HEADER_SIZE || esz % psz != 0 {
            return Err(Error::BadGeometry);
        }
        let sector_count = (cap / esz as u64) as u32;
        if sector_count < 3 {
            return Err(Error::BadGeometry);
        }
        let mut fs = Self {
            dev,
            index: heapless::Vec::new(),
            next_seq: 1,
            head: 0,
            active_sec: 0,
            free_sector: 0,
            sector_count,
            esz: esz as u32,
            psz: psz as u32,
        };
        fs.scan()?;
        // ¿Hay un sector activo parcial (último registro no terminó en frontera)?
        if fs.head != 0 && fs.head % fs.esz != 0 {
            fs.active_sec = fs.head / fs.esz;
            fs.free_sector = fs.find_free_sector(Some(fs.active_sec))?;
        } else {
            // Sin sector activo: arranca la escritura en una reserva fresca.
            let first = fs.find_free_sector(None)?;
            fs.head = first * fs.esz;
            fs.active_sec = first;
            fs.free_sector = fs.find_free_sector(Some(first))?;
        }
        Ok(fs)
    }

    /// Escribe (o sobrescribe) `value` bajo `key`.
    pub fn set(&mut self, key: &[u8], value: &[u8]) -> Result<(), Error<D::Error>> {
        if key.is_empty() || key.len() > MAX_KEY || value.len() > MAX_VALUE {
            return Err(Error::TooLarge);
        }
        let addr = self.append_record(key, value, false)?;
        self.upsert_index(key, self.next_seq - 1, addr, value.len() as u16, false)?;
        Ok(())
    }

    /// Borra `key` (escribe un tombstone). No falla si la clave no existía.
    pub fn delete(&mut self, key: &[u8]) -> Result<(), Error<D::Error>> {
        if key.is_empty() || key.len() > MAX_KEY {
            return Err(Error::TooLarge);
        }
        let addr = self.append_record(key, &[], true)?;
        self.upsert_index(key, self.next_seq - 1, addr, 0, true)?;
        Ok(())
    }

    /// Lee el valor de `key` en `buf`; devuelve los bytes leídos.
    pub fn get(&mut self, key: &[u8], buf: &mut [u8]) -> Result<usize, Error<D::Error>> {
        let (addr, key_len, val_len) = {
            let e = self
                .index
                .iter()
                .find(|e| !e.deleted && e.key.as_slice() == key)
                .ok_or(Error::NotFound)?;
            (e.addr, e.key.len(), e.val_len as usize)
        };
        if buf.len() < val_len {
            return Err(Error::BufferTooSmall);
        }
        let val_off = addr + HEADER_SIZE as u32 + key_len as u32;
        self.dev
            .read(val_off, &mut buf[..val_len])
            .map_err(Error::Device)?;
        Ok(val_len)
    }

    /// `true` si `key` existe (y no está borrada).
    pub fn contains(&self, key: &[u8]) -> bool {
        self.index
            .iter()
            .any(|e| !e.deleted && e.key.as_slice() == key)
    }

    /// Número de claves vivas.
    pub fn len(&self) -> usize {
        self.index.iter().filter(|e| !e.deleted).count()
    }

    /// `true` si no hay ninguna clave viva.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Invoca `f(key)` por cada clave viva (orden no garantizado).
    pub fn for_each_key<F: FnMut(&[u8])>(&self, mut f: F) {
        for e in self.index.iter().filter(|e| !e.deleted) {
            f(e.key.as_slice());
        }
    }

    /// Consume la FS y devuelve el [`BlockDevice`] subyacente (p. ej. para
    /// remontar o liberar el periférico).
    pub fn into_device(self) -> D {
        self.dev
    }

    // ---- internos --------------------------------------------------------

    /// Actualiza el índice respetando `seq` (gana el más reciente). Conserva
    /// tombstones para no resucitar claves al re-escanear en orden físico.
    fn upsert_index(
        &mut self,
        key: &[u8],
        seq: u32,
        addr: u32,
        val_len: u16,
        deleted: bool,
    ) -> Result<(), Error<D::Error>> {
        if let Some(e) = self.index.iter_mut().find(|e| e.key.as_slice() == key) {
            if e.seq >= seq {
                return Ok(());
            }
            e.seq = seq;
            e.addr = addr;
            e.val_len = val_len;
            e.deleted = deleted;
            return Ok(());
        }
        let mut k = heapless::Vec::new();
        k.extend_from_slice(key).map_err(|_| Error::TooLarge)?;
        self.index
            .push(Entry {
                key: k,
                seq,
                addr,
                val_len,
                deleted,
            })
            .map_err(|_| Error::IndexFull)?;
        Ok(())
    }

    /// Serializa y escribe un registro en `head`, garantizando hueco; devuelve
    /// su dirección base. Avanza `head` y `next_seq`.
    fn append_record(
        &mut self,
        key: &[u8],
        value: &[u8],
        delete: bool,
    ) -> Result<u32, Error<D::Error>> {
        let payload = key.len() + value.len();
        let rec_len = round_up(HEADER_SIZE + payload, self.psz as usize);
        if rec_len > self.esz as usize {
            return Err(Error::TooLarge);
        }
        self.ensure_room(rec_len as u32, false)?;
        let addr = self.head;

        let mut scratch = [0xFFu8; SCRATCH];
        let seq = self.next_seq;
        let flags = if delete { FLAG_DELETE } else { 0 };
        scratch[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        scratch[4..8].copy_from_slice(&seq.to_le_bytes());
        scratch[8..10].copy_from_slice(&(key.len() as u16).to_le_bytes());
        scratch[10..12].copy_from_slice(&(value.len() as u16).to_le_bytes());
        scratch[12..14].copy_from_slice(&flags.to_le_bytes());
        // 14..16 reservado (queda 0).
        scratch[16..20].copy_from_slice(&0u32.to_le_bytes()); // se rellena tras crc
        let hdr_crc = crc::crc32(&scratch[0..16]);
        scratch[16..20].copy_from_slice(&hdr_crc.to_le_bytes());
        scratch[20..24].copy_from_slice(&0u32.to_le_bytes());
        scratch[HEADER_SIZE..HEADER_SIZE + key.len()].copy_from_slice(key);
        scratch[HEADER_SIZE + key.len()..HEADER_SIZE + payload].copy_from_slice(value);
        let data_crc = crc::crc32(&scratch[HEADER_SIZE..HEADER_SIZE + payload]);
        scratch[20..24].copy_from_slice(&data_crc.to_le_bytes());

        self.dev
            .program(addr, &scratch[..rec_len])
            .map_err(Error::Device)?;
        self.head += rec_len as u32;
        self.next_seq += 1;
        Ok(addr)
    }

    /// Garantiza `rec_len` bytes contiguos en el sector activo; si no caben,
    /// rota a la reserva y repone otra. `in_compaction` evita recursión de GC.
    fn ensure_room(&mut self, rec_len: u32, in_compaction: bool) -> Result<(), Error<D::Error>> {
        let off = self.head % self.esz;
        // Cabe sólo si seguimos en el sector activo (cola borrada) y hay hueco.
        if self.head / self.esz == self.active_sec && off + rec_len <= self.esz {
            return Ok(());
        }
        // No cabe (o `head` derivó a otro sector): rota a la reserva (ya borrada).
        let entered = self.free_sector;
        self.head = entered * self.esz;
        self.active_sec = entered;
        self.free_sector = self.find_free_sector_inner(Some(entered), in_compaction)?;
        Ok(())
    }

    /// Busca un sector sin entradas vivas, lo borra y lo devuelve como reserva.
    fn find_free_sector(&mut self, exclude: Option<u32>) -> Result<u32, Error<D::Error>> {
        self.find_free_sector_inner(exclude, false)
    }

    fn find_free_sector_inner(
        &mut self,
        exclude: Option<u32>,
        in_compaction: bool,
    ) -> Result<u32, Error<D::Error>> {
        let head_sec = self.head / self.esz;
        for _attempt in 0..2 {
            for s in 0..self.sector_count {
                if Some(s) == exclude || s == head_sec {
                    continue;
                }
                if !self.sector_has_entries(s) {
                    self.dev.erase_sector(s * self.esz).map_err(Error::Device)?;
                    return Ok(s);
                }
            }
            if in_compaction {
                break; // sin GC anidado
            }
            if !self.compact(exclude, head_sec)? {
                break;
            }
        }
        Err(Error::NoSpace)
    }

    /// `true` si algún registro indexado (vivo o tombstone) vive en `sector`.
    fn sector_has_entries(&self, sector: u32) -> bool {
        let base = sector * self.esz;
        let end = base + self.esz;
        self.index.iter().any(|e| e.addr >= base && e.addr < end)
    }

    /// Compacta el sector víctima con MENOS entradas (≠ activo/reserva/excluido)
    /// reescribiendo sus registros hacia delante; luego queda reciclable.
    /// Devuelve `true` si movió algo (liberó potencialmente un sector).
    fn compact(&mut self, exclude: Option<u32>, head_sec: u32) -> Result<bool, Error<D::Error>> {
        // Elegir víctima: sector con >=1 entrada, mínimo recuento.
        let mut victim: Option<u32> = None;
        let mut best = usize::MAX;
        for s in 0..self.sector_count {
            if s == head_sec || Some(s) == exclude {
                continue;
            }
            let base = s * self.esz;
            let end = base + self.esz;
            let cnt = self
                .index
                .iter()
                .filter(|e| e.addr >= base && e.addr < end)
                .count();
            if cnt > 0 && cnt < best {
                best = cnt;
                victim = Some(s);
            }
        }
        let victim = match victim {
            Some(v) => v,
            None => return Ok(false),
        };
        let base = victim * self.esz;
        let end = base + self.esz;

        // Índices de entradas a mover (copiamos índices para no chocar con el
        // borrow mutable de append).
        let idxs: heapless::Vec<usize, N> = (0..self.index.len())
            .filter(|&i| self.index[i].addr >= base && self.index[i].addr < end)
            .collect();

        for i in idxs {
            let (seq, addr, key_len, val_len, deleted) = {
                let e = &self.index[i];
                (e.seq, e.addr, e.key.len(), e.val_len as usize, e.deleted)
            };
            // Releer key+value del registro original.
            let mut buf = [0u8; MAX_KEY + MAX_VALUE];
            let plen = key_len + val_len;
            self.dev
                .read(addr + HEADER_SIZE as u32, &mut buf[..plen])
                .map_err(Error::Device)?;
            // Re-emitir con el MISMO seq (no es una nueva versión lógica).
            let new_addr =
                self.append_with_seq(&buf[..key_len], &buf[key_len..plen], deleted, seq, true)?;
            self.index[i].addr = new_addr;
        }
        // Ahora la víctima ya no tiene entradas: bórrala.
        self.dev.erase_sector(base).map_err(Error::Device)?;
        Ok(true)
    }

    /// Como `append_record` pero con `seq` fijo (usado por la compactación) y
    /// sin tocar `next_seq`. `in_compaction` evita GC anidado.
    fn append_with_seq(
        &mut self,
        key: &[u8],
        value: &[u8],
        delete: bool,
        seq: u32,
        in_compaction: bool,
    ) -> Result<u32, Error<D::Error>> {
        let payload = key.len() + value.len();
        let rec_len = round_up(HEADER_SIZE + payload, self.psz as usize);
        self.ensure_room(rec_len as u32, in_compaction)?;
        let addr = self.head;

        let mut scratch = [0xFFu8; SCRATCH];
        let flags = if delete { FLAG_DELETE } else { 0 };
        scratch[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        scratch[4..8].copy_from_slice(&seq.to_le_bytes());
        scratch[8..10].copy_from_slice(&(key.len() as u16).to_le_bytes());
        scratch[10..12].copy_from_slice(&(value.len() as u16).to_le_bytes());
        scratch[12..14].copy_from_slice(&flags.to_le_bytes());
        let hdr_crc = crc::crc32(&scratch[0..16]);
        scratch[16..20].copy_from_slice(&hdr_crc.to_le_bytes());
        scratch[HEADER_SIZE..HEADER_SIZE + key.len()].copy_from_slice(key);
        scratch[HEADER_SIZE + key.len()..HEADER_SIZE + payload].copy_from_slice(value);
        let data_crc = crc::crc32(&scratch[HEADER_SIZE..HEADER_SIZE + payload]);
        scratch[20..24].copy_from_slice(&data_crc.to_le_bytes());

        self.dev
            .program(addr, &scratch[..rec_len])
            .map_err(Error::Device)?;
        self.head += rec_len as u32;
        Ok(addr)
    }

    /// Escanea todo el medio reconstruyendo el índice y dejando `head` tras el
    /// registro de mayor `seq`.
    fn scan(&mut self) -> Result<(), Error<D::Error>> {
        let mut max_seq = 0u32;
        let mut head_after = 0u32;
        for sec in 0..self.sector_count {
            let base = sec * self.esz;
            let mut off = 0u32;
            loop {
                if off + HEADER_SIZE as u32 > self.esz {
                    break;
                }
                let addr = base + off;
                let mut hdr = [0u8; HEADER_SIZE];
                self.dev.read(addr, &mut hdr).map_err(Error::Device)?;
                let magic = u32::from_le_bytes(hdr[0..4].try_into().unwrap());
                if magic != MAGIC {
                    break;
                }
                let stored_hcrc = u32::from_le_bytes(hdr[16..20].try_into().unwrap());
                let mut hc = hdr;
                hc[16..20].copy_from_slice(&0u32.to_le_bytes());
                if crc::crc32(&hc[0..16]) != stored_hcrc {
                    break; // cabecera rota -> fin del log en este sector
                }
                let seq = u32::from_le_bytes(hdr[4..8].try_into().unwrap());
                let key_len = u16::from_le_bytes(hdr[8..10].try_into().unwrap()) as usize;
                let val_len = u16::from_le_bytes(hdr[10..12].try_into().unwrap()) as usize;
                let flags = u16::from_le_bytes(hdr[12..14].try_into().unwrap());
                let data_crc = u32::from_le_bytes(hdr[20..24].try_into().unwrap());
                if key_len == 0 || key_len > MAX_KEY || val_len > MAX_VALUE {
                    break;
                }
                let payload = key_len + val_len;
                let rec_len = round_up(HEADER_SIZE + payload, self.psz as usize) as u32;
                if off + rec_len > self.esz {
                    break;
                }
                let mut pbuf = [0u8; MAX_KEY + MAX_VALUE];
                self.dev
                    .read(addr + HEADER_SIZE as u32, &mut pbuf[..payload])
                    .map_err(Error::Device)?;
                if crc::crc32(&pbuf[..payload]) != data_crc {
                    break; // payload roto (escritura truncada) -> fin del log
                }
                // Registro válido.
                let deleted = flags & FLAG_DELETE != 0;
                self.upsert_index(&pbuf[..key_len], seq, addr, val_len as u16, deleted)?;
                if seq >= max_seq {
                    max_seq = seq;
                    head_after = addr + rec_len;
                }
                off += rec_len;
            }
        }
        self.next_seq = max_seq + 1;
        self.head = head_after;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::RamFlash;

    const SEC: usize = 4096;
    const PG: usize = 256;

    fn fresh(sectors: usize) -> Rufs<RamFlash, 32> {
        Rufs::mount(RamFlash::new(sectors, SEC, PG)).unwrap()
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
        let dev = RamFlash::new(8, SEC, PG);
        let mut fs = Rufs::<_, 32>::mount(dev).unwrap();
        fs.set(b"a", b"1").unwrap();
        fs.set(b"b", b"2").unwrap();
        fs.set(b"a", b"3").unwrap();
        fs.delete(b"b").unwrap();
        let dev = fs.dev;
        // Remontar desde el mismo medio.
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
        let dev = RamFlash::new(8, SEC, PG);
        let mut fs = Rufs::<_, 32>::mount(dev).unwrap();
        fs.set(b"good", b"value").unwrap();
        let mut dev = fs.dev;

        // 2) Simula corte: el siguiente `program` se trunca a 100 bytes.
        dev.set_power_fail_after(100);
        let mut fs = Rufs::<_, 32>::mount(dev).unwrap();
        let r = fs.set(b"torn", &[0xAB; 200]); // debe fallar a mitad
        assert!(r.is_err());
        let dev = fs.dev;

        // 3) Remontar: "good" intacto, "torn" descartado por CRC.
        let mut fs2 = Rufs::<_, 32>::mount(dev).unwrap();
        let mut buf = [0u8; 16];
        let n = fs2.get(b"good", &mut buf).unwrap();
        assert_eq!(&buf[..n], b"value");
        assert_eq!(fs2.get(b"torn", &mut buf), Err(Error::NotFound));
    }

    #[test]
    fn wrap_and_compaction() {
        // Pocos sectores: forzar rotación + compactación reescribiendo la misma
        // clave muchas veces (cada versión es un registro nuevo).
        let mut fs = fresh(4);
        for i in 0..500u32 {
            fs.set(b"counter", &i.to_le_bytes()).unwrap();
        }
        let mut buf = [0u8; 4];
        let n = fs.get(b"counter", &mut buf).unwrap();
        assert_eq!(u32::from_le_bytes(buf[..n].try_into().unwrap()), 499);
        assert_eq!(fs.len(), 1);
        // Remonta y verifica que el último valor persiste tras tanta rotación.
        let dev = fs.dev;
        let mut fs2 = Rufs::<_, 32>::mount(dev).unwrap();
        let n = fs2.get(b"counter", &mut buf).unwrap();
        assert_eq!(u32::from_le_bytes(buf[..n].try_into().unwrap()), 499);
    }

    #[test]
    fn many_keys_survive_compaction() {
        let mut fs = fresh(6);
        for k in 0..20u8 {
            let key = [b'k', k];
            fs.set(&key, &[k; 100]).unwrap();
        }
        // Reescribe para generar basura y disparar GC.
        for _ in 0..30 {
            for k in 0..20u8 {
                let key = [b'k', k];
                fs.set(&key, &[k.wrapping_add(1); 120]).unwrap();
            }
        }
        let dev = fs.dev;
        let mut fs2 = Rufs::<_, 32>::mount(dev).unwrap();
        assert_eq!(fs2.len(), 20);
        for k in 0..20u8 {
            let key = [b'k', k];
            let mut buf = [0u8; 128];
            let n = fs2.get(&key, &mut buf).unwrap();
            assert!(buf[..n].iter().all(|&b| b == k.wrapping_add(1)));
        }
    }
}
