//! Comunicación serie — trait `SerialPort`.

/// Puerto serie (UART, USART, USB-CDC). Bloqueante en su forma básica.
///
/// Variantes async llegarán en hito G4 como traits separados
/// (`AsyncSerialRead` / `AsyncSerialWrite`) para no forzar el coste de
/// async en consumidores síncronos.
pub trait SerialPort {
    /// Tipo de error del bus.
    type Error;

    /// Escribe hasta `buf.len()` bytes. Devuelve cuántos se enviaron.
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error>;

    /// Lee hasta `buf.len()` bytes. Devuelve cuántos se recibieron.
    /// Implementaciones bloqueantes esperan al menos 1 byte.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error>;

    /// Drena el TX buffer; retorna cuando todo lo escrito ha salido.
    fn flush(&mut self) -> Result<(), Self::Error>;
}
