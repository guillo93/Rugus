//! GPIO digital — trait `GpioPin`.

/// Pin GPIO configurado como salida digital.
///
/// Implementaciones típicas tienen estado interno mutable (registro ODR
/// del chip) por lo que la mayoría de métodos toman `&mut self`.
pub trait GpioPin {
    /// Tipo de error (típicamente `core::convert::Infallible` para MCUs
    /// donde el set/clear no puede fallar).
    type Error;

    /// Pone el pin a nivel alto.
    fn set_high(&mut self) -> Result<(), Self::Error>;

    /// Pone el pin a nivel bajo.
    fn set_low(&mut self) -> Result<(), Self::Error>;

    /// Invierte el nivel del pin.
    fn toggle(&mut self) -> Result<(), Self::Error>;

    /// Lee el nivel actual del pin (lectura del ODR si está como salida,
    /// del IDR si está como entrada).
    fn is_high(&self) -> Result<bool, Self::Error>;
}
