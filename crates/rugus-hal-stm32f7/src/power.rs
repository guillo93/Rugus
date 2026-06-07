//! Gestión de energía — modo STOP con wake por RTC (F5.A.2) en STM32F769I-DISCO.
//!
//! Implementa el sueño profundo de la línea de energía de Rugus: cuando el
//! scheduler decide que el núcleo puede dormir un plazo largo, en vez de un
//! `wfi` con tick dinámico (que mantiene HSE/PLL vivos) se entra en **STOP**,
//! apagando HSE y PLL. El **wakeup timer del RTC**, relojado por **LSI**, es la
//! única base que sigue corriendo en STOP y reprograma el despertar al plazo
//! pedido. Al salir, [`crate::rcc::restore_after_stop`] re-arma HSE/PLL/216 MHz.
//!
//! **Precisión:** durante el STOP el tiempo lo lleva LSI (~32 kHz, ±5 %), no
//! HSE/PLL, así que el reloj monotónico avanza con esa tolerancia SOLO mientras
//! se duerme; en RUN se conserva la exactitud del tick dinámico (F5.A.1).
//!
//! **Wake source:** RTC wakeup → EXTI línea 22. El `wfi` se ejecuta con las IRQs
//! enmascaradas (`PRIMASK`), de modo que el evento despierta al núcleo SIN entrar
//! a la ISR `RTC_WKUP` (no necesitamos un handler ni que corra código de IRQ): al
//! volver se limpian los flags y se restauran los relojes en línea.

use crate::pac;

/// Frecuencia nominal de LSI (RM0410 §6.2.5): ~32 kHz.
const LSI_HZ: u32 = 32_000;
/// LSI / 16 vía el divisor del wakeup timer (WUCKSEL=Div16) → 2 kHz.
const WUT_HZ: u32 = LSI_HZ / 16; // 2000 Hz → 0.5 ms por tick
/// WUTR es de 16 bits: acota el máximo de ms por STOP (~32.7 s).
const WUT_MAX_TICKS: u32 = 0xFFFF;

/// Llaves de desbloqueo del registro protegido del RTC (RM0410 §29.4.22).
const WPR_KEY1: u8 = 0xCA;
const WPR_KEY2: u8 = 0x53;
/// Cualquier valor distinto re-bloquea la escritura del RTC.
const WPR_LOCK: u8 = 0xFF;

/// Configura el RTC (LSI) y la línea EXTI de wakeup para usar STOP.
///
/// Debe llamarse una vez tras `rcc::init` (que ya habilitó el reloj de PWR).
/// Resetea el dominio de backup solo si el RTC no estaba ya en LSI, para no
/// perder la configuración entre arranques en caliente.
pub fn init(dp: &pac::Peripherals) {
    let rcc = &dp.RCC;
    let pwr = &dp.PWR;

    // LSI ON (oscilador de baja potencia que sobrevive a STOP).
    rcc.csr.modify(|_, w| w.lsion().set_bit());
    while !rcc.csr.read().lsirdy().bit() {}

    // Acceso de escritura al dominio de backup (RTC/BDCR).
    pwr.cr1.modify(|_, w| w.dbp().set_bit());

    // Selecciona LSI como reloj del RTC. RTCSEL solo se puede cambiar tras un
    // reset del dominio de backup, así que solo lo hacemos si no estaba en LSI.
    if !rcc.bdcr.read().rtcsel().is_lsi() {
        rcc.bdcr.modify(|_, w| w.bdrst().set_bit());
        rcc.bdcr.modify(|_, w| w.bdrst().clear_bit());
        rcc.bdcr.modify(|_, w| w.rtcsel().lsi());
    }
    rcc.bdcr.modify(|_, w| w.rtcen().set_bit());

    // EXTI línea 22 = RTC wakeup: desenmascarar + flanco de subida.
    dp.EXTI.imr.modify(|_, w| w.mr22().set_bit());
    dp.EXTI.rtsr.modify(|_, w| w.tr22().set_bit());

    // El NVIC debe tener el IRQ habilitado para que el evento sea condición de
    // wakeup del `wfi`; como dormimos con PRIMASK=1, la ISR no llega a ejecutar.
    // SAFETY: arranque single-thread; RTC_WKUP es una IRQ de wakeup controlada.
    unsafe {
        cortex_m::peripheral::NVIC::unmask(pac::Interrupt::RTC_WKUP);
    }
}

/// Mantiene la conexión de depuración (SWD/RTT) viva durante STOP.
///
/// STOP detiene el reloj del núcleo y, por defecto, la sonda pierde el canal
/// RTT. Activar `DBGMCU_CR.DBG_STOP` mantiene el dominio de depuración alimentado
/// (a costa de más consumo), útil para validar en banco. NO usar en producción.
pub fn keep_debug_in_stop(dp: &pac::Peripherals) {
    dp.DBGMCU.cr.modify(|_, w| w.dbg_stop().set_bit());
}

/// Convierte ms a ticks del wakeup timer, acotado al rango del WUTR de 16 bits.
#[inline]
fn ms_to_ticks(ms: u32) -> u32 {
    (ms.saturating_mul(WUT_HZ) / 1000).clamp(1, WUT_MAX_TICKS)
}

/// Entra en STOP durante `ms` (acotado a ~32.7 s) y devuelve los ms reales
/// dormidos según el wakeup timer.
///
/// Pensada para registrarse como manejador de STOP del backend arch
/// (`time::set_stop_handler`): el `idle_until` del tick dinámico la invoca cuando
/// el próximo plazo del scheduler supera el umbral configurado.
pub fn enter_stop_ms(ms: u32) -> u32 {
    let ticks = ms_to_ticks(ms);
    // ms realmente representados por esos ticks (para sumar al reloj monotónico).
    let real_ms = ticks.saturating_mul(1000) / WUT_HZ;

    // SAFETY: corre en el camino de idle del scheduler (single-core). El RTC y
    // EXTI ya fueron configurados por `init`; aquí solo se reprograma el WUT y se
    // entra/sale de STOP. Robar los periféricos es seguro porque ningún otro
    // contexto los toca mientras el núcleo está dormido.
    let dp = unsafe { pac::Peripherals::steal() };
    let mut cp = unsafe { cortex_m::Peripherals::steal() };
    let rtc = &dp.RTC;

    // Toda la secuencia con IRQs enmascaradas: el wakeup del RTC despierta el
    // `wfi` pero, al estar PRIMASK=1, NO se entra a la ISR (no hay handler).
    cortex_m::interrupt::free(|_| {
        // Programa el wakeup timer: desbloquear WPR, parar WUT, fijar cuenta.
        rtc.wpr.write(|w| w.key().bits(WPR_KEY1));
        rtc.wpr.write(|w| w.key().bits(WPR_KEY2));
        rtc.cr.modify(|_, w| w.wute().clear_bit());
        while !rtc.isr.read().wutwf().bit() {}
        rtc.wutr.write(|w| w.wut().bits((ticks - 1) as u16));
        rtc.cr.modify(|_, w| w.wucksel().div16());
        rtc.isr.modify(|_, w| w.wutf().clear_bit());
        rtc.cr.modify(|_, w| w.wutie().set_bit().wute().set_bit());
        rtc.wpr.write(|w| w.key().bits(WPR_LOCK));

        // Limpia un posible pending previo de la línea EXTI 22.
        dp.EXTI.pr.write(|w| w.pr22().set_bit());

        // Configura STOP: regulador en baja potencia (LPDS), NO standby (PDDS).
        dp.PWR
            .cr1
            .modify(|_, w| w.pdds().stop_mode().lpds().set_bit());
        // SLEEPDEEP=1 hace que WFI entre en STOP en vez de SLEEP normal.
        cp.SCB.set_sleepdeep();
        cortex_m::asm::dsb();
        cortex_m::asm::wfi();
        // --- el núcleo despierta aquí (HSI 16 MHz, HSE/PLL apagados) ---
        cp.SCB.clear_sleepdeep();

        // Para el wakeup timer y limpia flags (RTC + EXTI + NVIC pending).
        rtc.wpr.write(|w| w.key().bits(WPR_KEY1));
        rtc.wpr.write(|w| w.key().bits(WPR_KEY2));
        rtc.cr
            .modify(|_, w| w.wute().clear_bit().wutie().clear_bit());
        rtc.isr.modify(|_, w| w.wutf().clear_bit());
        rtc.wpr.write(|w| w.key().bits(WPR_LOCK));
        dp.EXTI.pr.write(|w| w.pr22().set_bit());
        cortex_m::peripheral::NVIC::unpend(pac::Interrupt::RTC_WKUP);

        // Restaura HSE/PLL/216 MHz antes de devolver el control al scheduler.
        crate::rcc::restore_after_stop(&dp);
    });

    real_ms
}
