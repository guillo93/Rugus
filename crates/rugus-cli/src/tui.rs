//! TUI con ratatui: consola con scrollback estilado, panel de léxico e input.
//!
//! Aquí vive el "wow": tablas, colores y paneles. El kernel se mantiene serio;
//! la presentación rica es del host.

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color as RColor, Modifier, Style as RStyle};
use ratatui::text::{Line, Span as RSpan};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::{Frame, Terminal};

use rugus_proto::command::LEXICON;
use rugus_proto::render::{BaseColor, Color};
use rugus_proto::{Command, LineAssembler, StyledLine};

use crate::device::Device;

type Backend = CrosstermBackend<Stdout>;

/// Estado de la TUI durante una sesión.
struct App {
    device: Device,
    assembler: LineAssembler,
    scrollback: Vec<Line<'static>>,
    input: String,
    should_quit: bool,
}

impl App {
    fn new(device: Device) -> Self {
        let mut app = App {
            device,
            assembler: LineAssembler::new(),
            scrollback: Vec::new(),
            input: String::new(),
            should_quit: false,
        };
        app.push_system(format!(
            "Conectado: {} · {}",
            app.device.kind.label(),
            app.device.signature.label()
        ));
        app.push_system("Escribe un comando y pulsa Enter. Esc o Ctrl-C para salir.".into());
        app
    }

    fn push_system(&mut self, text: String) {
        self.scrollback.push(Line::from(vec![RSpan::styled(
            text,
            RStyle::default()
                .fg(RColor::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )]));
    }

    /// Vuelca bytes recibidos del dispositivo al scrollback como líneas estiladas.
    fn drain_device(&mut self) {
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        while let Ok(chunk) = self.device.bytes_rx.try_recv() {
            chunks.push(chunk);
        }
        for chunk in chunks {
            for line in self.assembler.push(&chunk) {
                let styled = StyledLine::parse(&line);
                self.scrollback.push(styled_to_line(&styled));
            }
        }
    }

    fn submit(&mut self) {
        let raw = self.input.trim().to_string();
        if raw.is_empty() {
            return;
        }
        let cmd = Command::parse(&raw);
        // Eco local del comando enviado.
        let marker = if cmd.is_known() {
            RColor::Green
        } else {
            RColor::Yellow
        };
        self.scrollback.push(Line::from(vec![
            RSpan::styled("› ", RStyle::default().fg(marker)),
            RSpan::raw(raw.clone()),
        ]));
        if !self.device.send(cmd.to_wire()) {
            self.push_system("(transporte cerrado: no se pudo enviar)".into());
        }
        self.input.clear();
    }

    fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        match code {
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => self.should_quit = true,
            KeyCode::Esc => self.should_quit = true,
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            _ => {}
        }
    }
}

/// Ejecuta la TUI de sesión hasta que el usuario sale.
pub fn run(device: Device) -> Result<()> {
    let mut terminal = setup()?;
    let mut app = App::new(device);

    let res = (|| -> Result<()> {
        while !app.should_quit {
            app.drain_device();
            terminal.draw(|f| draw(f, &app))?;
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        app.on_key(key.code, key.modifiers);
                    }
                }
            }
        }
        Ok(())
    })();

    restore(&mut terminal)?;
    res
}

fn setup() -> Result<Terminal<Backend>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    Ok(terminal)
}

fn restore(terminal: &mut Terminal<Backend>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(f.area());

    draw_header(f, chunks[0], app);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(chunks[1]);

    draw_console(f, body[0], app);
    draw_lexicon(f, body[1]);
    draw_input(f, chunks[2], app);
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let sig = &app.device.signature;
    let header = Line::from(vec![
        RSpan::styled(
            " RUGUS ",
            RStyle::default()
                .fg(RColor::Black)
                .bg(RColor::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        RSpan::raw("  "),
        RSpan::styled(
            format!("tier {}", sig.tier),
            RStyle::default().fg(RColor::LightCyan),
        ),
        RSpan::raw("  ·  "),
        RSpan::styled(
            format!("chip {}", sig.chip),
            RStyle::default().fg(RColor::LightMagenta),
        ),
        RSpan::raw("  ·  "),
        RSpan::styled(
            format!("shell {}", sig.shell),
            RStyle::default().fg(RColor::LightGreen),
        ),
        RSpan::raw("  ·  "),
        RSpan::styled(
            format!("cli {}", sig.cli),
            RStyle::default().fg(RColor::White),
        ),
        RSpan::raw("  ·  "),
        RSpan::styled(
            format!("proto {}", sig.proto),
            RStyle::default().fg(RColor::Gray),
        ),
        RSpan::raw("  ·  "),
        RSpan::styled(
            app.device.kind.label(),
            RStyle::default().fg(RColor::Yellow),
        ),
    ]);
    let block = Block::default().borders(Borders::ALL).title(" rugus-cli ");
    f.render_widget(Paragraph::new(header).block(block), area);
}

fn draw_console(f: &mut Frame, area: Rect, app: &App) {
    let inner_height = area.height.saturating_sub(2) as usize;
    let total = app.scrollback.len();
    let start = total.saturating_sub(inner_height);
    let visible: Vec<Line> = app.scrollback[start..].to_vec();
    let block = Block::default().borders(Borders::ALL).title(" consola ");
    f.render_widget(Paragraph::new(visible).block(block), area);
}

fn draw_lexicon(f: &mut Frame, area: Rect) {
    let rows: Vec<Row> = LEXICON
        .iter()
        .map(|(verb, help)| {
            Row::new(vec![
                Cell::from(RSpan::styled(
                    *verb,
                    RStyle::default()
                        .fg(RColor::LightCyan)
                        .add_modifier(Modifier::BOLD),
                )),
                Cell::from(*help),
            ])
        })
        .collect();
    let table = Table::new(rows, [Constraint::Length(10), Constraint::Min(10)])
        .header(
            Row::new(vec!["verbo", "acción"]).style(
                RStyle::default()
                    .fg(RColor::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" léxico rush "),
        );
    f.render_widget(table, area);
}

fn draw_input(f: &mut Frame, area: Rect, app: &App) {
    let line = Line::from(vec![
        RSpan::styled(
            "› ",
            RStyle::default()
                .fg(RColor::Green)
                .add_modifier(Modifier::BOLD),
        ),
        RSpan::raw(app.input.as_str()),
        RSpan::styled("▏", RStyle::default().fg(RColor::Green)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" comando — Enter envía · Esc sale ");
    f.render_widget(Paragraph::new(line).block(block), area);
}

/// Convierte una `StyledLine` del protocolo en una `Line` de ratatui.
fn styled_to_line(styled: &StyledLine) -> Line<'static> {
    let spans: Vec<RSpan<'static>> = styled
        .spans
        .iter()
        .map(|s| {
            let mut style = RStyle::default().fg(map_color(s.style.fg));
            if s.style.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            RSpan::styled(s.text.clone(), style)
        })
        .collect();
    if spans.is_empty() {
        Line::from("")
    } else {
        Line::from(spans)
    }
}

fn map_color(c: Color) -> RColor {
    match c {
        Color::Default => RColor::Reset,
        Color::Black => RColor::Black,
        Color::Red => RColor::Red,
        Color::Green => RColor::Green,
        Color::Yellow => RColor::Yellow,
        Color::Blue => RColor::Blue,
        Color::Magenta => RColor::Magenta,
        Color::Cyan => RColor::Cyan,
        Color::White => RColor::Gray,
        Color::Bright(b) => match b {
            BaseColor::Black => RColor::DarkGray,
            BaseColor::Red => RColor::LightRed,
            BaseColor::Green => RColor::LightGreen,
            BaseColor::Yellow => RColor::LightYellow,
            BaseColor::Blue => RColor::LightBlue,
            BaseColor::Magenta => RColor::LightMagenta,
            BaseColor::Cyan => RColor::LightCyan,
            BaseColor::White => RColor::White,
        },
    }
}
