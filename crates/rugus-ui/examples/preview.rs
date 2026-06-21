//! Vista previa host del sistema visual `rugus-ui`: renderiza los verbos de la
//! consola `rush` (cosmos/ecosystem/coil/scar/letargo) con datos simulados,
//! usando exactamente las mismas primitivas del [`Painter`] que la personalidad
//! full emite en el dispositivo. Sirve de sign-off visual sin placa ni serie:
//!
//! ```sh
//! cargo run -p rugus-ui --example preview
//! NO_COLOR=1 cargo run -p rugus-ui --example preview   # fidelidad plana 7-bit
//! ```

use rugus_ui::{set_color, Painter, Role};

fn show(label: &str, f: impl FnOnce(&mut Painter)) {
    let mut buf = [0u8; 1024];
    let mut p = Painter::new(&mut buf);
    f(&mut p);
    let n = p.len();
    print!("{}", core::str::from_utf8(&buf[..n]).unwrap());
    let _ = label;
}

fn cosmos(p: &mut Painter) {
    p.header("cosmos");
    p.text(Role::Focus, "f407vet6").raw("  ");
    p.badge(Role::Core, " full ").raw(" ");
    p.badge(Role::Data, " tier:full ").raw("\r\n");
    p.kvn("arranques", Role::Data, 7).raw("   ");
    p.kvn("tareas", Role::Data, 3).raw("\r\n");
}

fn ecosystem(p: &mut Painter) {
    p.header("ecosystem");
    p.badge(Role::Core, " sano ").raw("  \r\n");
    p.kvn("tareas", Role::Data, 3).raw("   ");
    p.kvn("faults", Role::Core, 0).raw("\r\n");
    p.kv("reset", Role::Data, "power-on").raw("\r\n");
}

fn letargo(p: &mut Painter) {
    p.header("letargo");
    p.kvn("uptime", Role::Data, 412_356)
        .text(Role::Chrome, " ms\r\n");
    p.on(Role::Chrome).raw("idle   ").off();
    p.meter(100 - 94, 16).raw(" ");
    p.num(Role::Core, 94).text(Role::Chrome, "% ocio\r\n");
    p.kvn("systick", Role::Data, 412).raw("   ");
    p.kvn("stop", Role::Data, 38).raw("\r\n");
}

fn coil(p: &mut Painter) {
    p.header("coil");
    p.text(Role::Chrome, "  # pri  modo  estado    pila\r\n");
    let rows = [
        (0u32, 0u32, false, "run", 41u32),
        (1, 1, false, "sleep", 18),
        (2, 2, true, "ready", 73),
        (3, 2, true, "blocked", 95),
    ];
    for (idx, pri, user, st, pct) in rows {
        p.raw("  ").num(Role::Data, idx).raw("  ");
        p.num(Role::Data, pri).raw("  ");
        if user {
            p.text(Role::Text, "user");
        } else {
            p.text(Role::Focus, "kern");
        }
        p.raw("  ");
        let st_role = match st {
            "run" | "ready" => Role::Core,
            "dead" | "killed" => Role::Fault,
            _ => Role::Warn,
        };
        p.on(st_role).raw(st).off();
        for _ in st.len()..9 {
            p.raw(" ");
        }
        let pr = if pct >= 90 {
            Role::Fault
        } else if pct >= 70 {
            Role::Warn
        } else {
            Role::Core
        };
        p.meter(pct, 8)
            .raw(" ")
            .num(pr, pct)
            .text(Role::Chrome, "%\r\n");
    }
}

fn scar(p: &mut Painter) {
    p.header("scar");
    p.kvn("arranques", Role::Data, 7).raw("   ");
    p.kvn("faults", Role::Fault, 2).raw("\r\n");
    p.on(Role::Chrome).raw("  task ").off();
    p.num(Role::Data, 3).raw(": ");
    p.num(Role::Fault, 2).text(Role::Chrome, " faults\r\n");
    p.on(Role::Fault).raw("\u{2717} ultimo  ").off();
    p.kvn("kind", Role::Fault, 1).raw("  ");
    p.kvn("task", Role::Data, 3).raw("  ");
    p.on(Role::Chrome).raw("pc=").off().text(Role::Data, "0x");
    p.on(Role::Data).hex(0x0800_1a2c).off().raw("  ");
    p.on(Role::Chrome).raw("addr=").off().text(Role::Data, "0x");
    p.on(Role::Data).hex(0x2002_3000).off().raw("\r\n");
}

fn prompt(p: &mut Painter) {
    p.on(Role::Core).raw("rugus").off();
    p.text(Role::Chrome, ":").text(Role::Focus, "f407vet6");
    p.raw(" ").on(Role::Core).raw("\u{25b8}").off().raw(" ");
}

fn main() {
    if std::env::var_os("NO_COLOR").is_some() {
        set_color(false);
    }
    let nl = "\r\n";
    print!("{nl}");
    show("cosmos", cosmos);
    print!("{nl}");
    show("ecosystem", ecosystem);
    print!("{nl}");
    show("letargo", letargo);
    print!("{nl}");
    show("coil", coil);
    print!("{nl}");
    show("scar", scar);
    print!("{nl}");
    show("prompt", prompt);
    print!("knock{nl}{nl}");
}
