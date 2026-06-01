//! TUI — ratatui 이벤트 루프. 리스트 + 본문검색 + 인프로세스 프리뷰(캐시) + enter→resume.
//!
//! 설계: 키 핸들링은 순수 함수 `App::handle_key`(터미널 없이 단위테스트), 렌더는 `App::render`
//! (TestBackend 로 검증). 이벤트 루프(`run`)만 crossterm 에 의존. resume 는 터미널 복원 **후**
//! main 이 수행(exec 가 프로세스를 교체하므로 raw mode 를 먼저 해제). [D7/D14]
//!
//! MVP 범위: flat 리스트 + substring-AND 검색 + 프리뷰 + resume/copy + 스크롤.
//! 계층 트리(tab)·스코프(ctrl-g)·구문 하이라이트(syntect)는 dogfooding 후속.

use crate::index::SessionRow;
use crate::model::{self, Source};
use crate::{preview, search};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::collections::HashMap;

/// TUI 종료 결과. main 이 터미널 복원 후 처리.
pub enum Outcome {
    Quit,
    Resume(SessionRow),
    Copy(SessionRow),
}

pub struct App {
    rows: Vec<SessionRow>,
    haystacks: Vec<String>,
    query: String,
    filtered: Vec<usize>, // rows 인덱스
    sel: usize,           // filtered 인덱스
    preview_cache: HashMap<String, Vec<Line<'static>>>,
    preview_scroll: u16,
    now: i64,
}

impl App {
    pub fn new(rows: Vec<SessionRow>, now: i64) -> Self {
        let haystacks = search::build_haystacks(&rows);
        let filtered = (0..rows.len()).collect();
        App {
            rows,
            haystacks,
            query: String::new(),
            filtered,
            sel: 0,
            preview_cache: HashMap::new(),
            preview_scroll: 0,
            now,
        }
    }

    fn recompute(&mut self) {
        self.filtered = search::filter(&self.haystacks, &self.query);
        if self.sel >= self.filtered.len() {
            self.sel = self.filtered.len().saturating_sub(1);
        }
        self.preview_scroll = 0;
    }

    fn current(&self) -> Option<&SessionRow> {
        self.filtered.get(self.sel).map(|&i| &self.rows[i])
    }

    /// 순수 키 핸들러. 종료 액션이면 Some(Outcome).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Outcome> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Some(Outcome::Quit),
            KeyCode::Char('c') if ctrl => return Some(Outcome::Quit),
            KeyCode::Enter => {
                if let Some(r) = self.current() {
                    return Some(Outcome::Resume(r.clone()));
                }
            }
            KeyCode::Char('y') if ctrl => {
                if let Some(r) = self.current() {
                    return Some(Outcome::Copy(r.clone()));
                }
            }
            KeyCode::Down => self.move_sel(1),
            KeyCode::Char('n') if ctrl => self.move_sel(1),
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('p') if ctrl => self.move_sel(-1),
            KeyCode::Char('d') if ctrl => self.preview_scroll = self.preview_scroll.saturating_add(10),
            KeyCode::Char('u') if ctrl => self.preview_scroll = self.preview_scroll.saturating_sub(10),
            KeyCode::Backspace => {
                self.query.pop();
                self.recompute();
            }
            KeyCode::Char(c) if !ctrl => {
                self.query.push(c);
                self.recompute();
            }
            _ => {}
        }
        None
    }

    fn move_sel(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            return;
        }
        let max = self.filtered.len() - 1;
        let next = (self.sel as i32 + delta).clamp(0, max as i32) as usize;
        if next != self.sel {
            self.sel = next;
            self.preview_scroll = 0;
        }
    }

    /// 현재 선택 세션의 프리뷰 라인(캐시). 렌더에서 호출.
    fn preview_lines(&mut self) -> Vec<Line<'static>> {
        let Some(row) = self.current() else {
            return vec![Line::from("")];
        };
        let path = row.path.clone();
        let source = if row.source == "codex" {
            Source::Codex
        } else {
            Source::Claude
        };
        self.preview_cache
            .entry(path.clone())
            .or_insert_with(|| preview::render_session(&path, source))
            .clone()
    }

    pub fn render(&mut self, f: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(f.area());

        // 검색 바
        let count = format!("{}/{}", self.filtered.len(), self.rows.len());
        let header = Line::from(vec![
            Span::styled("search> ", Style::default().fg(Color::Rgb(78, 201, 176))),
            Span::raw(self.query.clone()),
            Span::styled(
                format!("    [{count}]  enter=resume · ctrl-y=copy · ctrl-d/u=scroll · esc=quit"),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]);
        f.render_widget(Paragraph::new(header), chunks[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(chunks[1]);

        // 리스트
        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .map(|&i| {
                let r = &self.rows[i];
                let rt = model::reltime(r.mtime, self.now);
                let tag = if r.source == "codex" { "ⓒ" } else { " " };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{rt:>4} "), Style::default().add_modifier(Modifier::DIM)),
                    Span::styled(format!("{tag} "), Style::default().fg(Color::Rgb(78, 201, 176))),
                    Span::raw(truncate(&r.title, 48)),
                    Span::styled(
                        format!("  {}", r.proj),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ]))
            })
            .collect();
        let mut state = ListState::default();
        if !self.filtered.is_empty() {
            state.select(Some(self.sel));
        }
        let list = List::new(items)
            .block(Block::default().borders(Borders::RIGHT))
            .highlight_style(Style::default().bg(Color::Rgb(40, 44, 52)).add_modifier(Modifier::BOLD))
            .highlight_symbol("▌");
        f.render_stateful_widget(list, body[0], &mut state);

        // 프리뷰
        let title = self
            .current()
            .map(|r| format!(" {} {} ", if r.source == "codex" { "[codex]" } else { "[claude]" }, r.proj))
            .unwrap_or_default();
        let lines = self.preview_lines();
        let preview = Paragraph::new(lines)
            .block(Block::default().title(title))
            .scroll((self.preview_scroll, 0));
        f.render_widget(preview, body[1]);
    }
}

/// codepoint 기준 자르기.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

/// 이벤트 루프 실행. 터미널 setup/teardown 포함. 반환 시 터미널은 이미 복원됨.
pub fn run(rows: Vec<SessionRow>, now: i64) -> Outcome {
    let mut term = ratatui::init();
    let mut app = App::new(rows, now);
    let outcome = loop {
        let _ = term.draw(|f| app.render(f));
        match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                if let Some(o) = app.handle_key(k) {
                    break o;
                }
            }
            Ok(_) => {}
            Err(_) => break Outcome::Quit,
        }
    };
    ratatui::restore();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rows() -> Vec<SessionRow> {
        vec![
            mkrow("claude", "alpha auth jwt", "work/a", 300),
            mkrow("codex", "beta deploy", "work/b", 200),
            mkrow("claude", "gamma auth token", "work/c", 100),
        ]
    }
    fn mkrow(src: &str, title: &str, proj: &str, mtime: i64) -> SessionRow {
        SessionRow {
            source: src.into(),
            id: "id".into(),
            cwd: Some("/tmp".into()),
            cwd_lossy: false,
            path: "/no/such/path.jsonl".into(),
            mtime,
            title: title.into(),
            proj: proj.into(),
            body: String::new(),
        }
    }
    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn typing_filters_and_resets() {
        let mut app = App::new(rows(), 1000);
        assert_eq!(app.filtered.len(), 3);
        for c in "auth".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.query, "auth");
        assert_eq!(app.filtered.len(), 2); // alpha, gamma
        // AND
        for c in " jwt".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.filtered.len(), 1);
        // backspace 복구
        for _ in 0..4 {
            app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        }
        assert_eq!(app.query, "auth");
        assert_eq!(app.filtered.len(), 2);
    }

    #[test]
    fn navigation_and_enter_outcome() {
        let mut app = App::new(rows(), 1000);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.sel, 1);
        app.handle_key(ctrl('n'));
        assert_eq!(app.sel, 2);
        app.handle_key(ctrl('n')); // clamp at max
        assert_eq!(app.sel, 2);
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.sel, 1);
        // enter → Resume(현재 행)
        match app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) {
            Some(Outcome::Resume(r)) => assert_eq!(r.title, "beta deploy"),
            _ => panic!("expected Resume"),
        }
        // ctrl-y → Copy
        assert!(matches!(app.handle_key(ctrl('y')), Some(Outcome::Copy(_))));
        // esc → Quit
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(Outcome::Quit)
        ));
    }

    #[test]
    fn renders_to_backend() {
        let backend = TestBackend::new(100, 20);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new(rows(), 1000);
        term.draw(|f| app.render(f)).unwrap();
        // 버퍼에 검색 프롬프트 + 첫 행 제목 일부가 보여야.
        let buf = term.backend().buffer();
        let flat: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(flat.contains("search>"));
        assert!(flat.contains("alpha"));
    }
}
