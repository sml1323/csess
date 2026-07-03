//! TUI — ratatui 이벤트 루프. 리스트 + 본문검색 + 계층 트리(tab)/스코프(ctrl-g) + 인프로세스 프리뷰 + enter→resume.
//!
//! 설계: 키 핸들링은 순수 함수 `App::handle_key`(터미널 없이 단위테스트), 렌더는 `App::render`
//! (TestBackend 로 검증). 이벤트 루프(`run`)만 crossterm 에 의존. resume 는 터미널 복원 **후**
//! main 이 수행(exec 가 프로세스를 교체하므로 raw mode 를 먼저 해제). [D7/D14]
//!
//! 뷰 상태: `view`(Flat/Tree) · `prefix`(트리 드릴 위치, 빈=ROOT=HOME) · `scope`(프로젝트 한정).
//! 리스트 항목은 [`crate::tree::ListItem`](UP/Dir/Session) 혼합 — 트리 빌더는 `tree` 모듈(순수).

use crate::index::SessionRow;
use crate::model::{self, Source};
use crate::tree::{self, ListItem, ViewMode};
use crate::{preview, search, theme};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Position};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem as UiListItem, ListState, Paragraph};
use ratatui::Frame;
use std::collections::HashMap;
use unicode_width::UnicodeWidthStr;

/// 소스 필터(ctrl-s 토글) — 전체 / Claude만 / Codex만. flat·tree 양쪽에 적용. [D14 후속]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceFilter {
    All,
    Claude,
    Codex,
}

impl SourceFilter {
    fn next(self) -> Self {
        match self {
            SourceFilter::All => SourceFilter::Claude,
            SourceFilter::Claude => SourceFilter::Codex,
            SourceFilter::Codex => SourceFilter::All,
        }
    }
    /// build_tree_level/필터용 소스 문자열. All=None.
    fn as_str(self) -> Option<&'static str> {
        match self {
            SourceFilter::All => None,
            SourceFilter::Claude => Some("claude"),
            SourceFilter::Codex => Some("codex"),
        }
    }
}

/// TUI 종료 결과. main 이 터미널 복원 후 처리.
pub enum Outcome {
    Quit,
    Resume(SessionRow),
    Copy(SessionRow),
}

/// 앵커 시 매치 줄 위로 함께 보여줄 맥락 줄 수. [D7]
const LEAD_IN: u16 = 2;

pub struct App {
    rows: Vec<SessionRow>,
    haystacks: Vec<String>,
    query: String,
    items: Vec<ListItem>, // 현재 보이는 행(Flat=세션만, Tree=UP/Dir/세션 혼합)
    sel: usize,           // items 인덱스
    preview_cache: HashMap<String, Vec<Line<'static>>>,
    hl_cache: HashMap<String, Vec<Line<'static>>>, // (path,query) → 하이라이트 라인. 스크롤 idle 유지. [D6]
    preview_scroll: u16,
    now: i64,
    home: String,         // $HOME (트리 루트)
    view: ViewMode,
    prefix: String,       // 트리 드릴 prefix, 빈=ROOT(=home)
    scope: Option<String>, // Some(cwd)=flat 뷰를 그 프로젝트로 한정
    source_filter: SourceFilter, // 전체/Claude만/Codex만 (ctrl-s)
}

impl App {
    pub fn new(rows: Vec<SessionRow>, now: i64, home: String) -> Self {
        let haystacks = search::build_haystacks(&rows);
        let mut app = App {
            rows,
            haystacks,
            query: String::new(),
            items: Vec::new(),
            sel: 0,
            preview_cache: HashMap::new(),
            hl_cache: HashMap::new(),
            preview_scroll: 0,
            now,
            home,
            view: ViewMode::Flat,
            prefix: String::new(),
            scope: None,
            source_filter: SourceFilter::All,
        };
        app.recompute();
        app
    }

    /// `self.items` 단일 진입점. 뷰에 따라 분기 + sel 클램프 + 프리뷰 스크롤 리셋.
    fn recompute(&mut self) {
        self.items = match self.view {
            ViewMode::Flat => self.recompute_flat(),
            ViewMode::Tree => self.recompute_tree(),
        };
        if self.sel >= self.items.len() {
            self.sel = self.items.len().saturating_sub(1);
        }
        self.preview_scroll = self.anchor_for_selection();
    }

    /// Flat: 소스 필터 → 스코프 필터 → 본문 AND 검색(mtime desc 보존). 전부 Session 항목.
    fn recompute_flat(&self) -> Vec<ListItem> {
        let src = self.source_filter.as_str();
        search::filter(&self.haystacks, &self.query)
            .into_iter()
            .filter(|&i| {
                let r = &self.rows[i];
                src.is_none_or(|s| r.source == s)
                    && match &self.scope {
                        Some(sc) => r.cwd.as_deref() == Some(sc.as_str()),
                        None => true,
                    }
            })
            .map(|i| ListItem::Session { row_idx: i })
            .collect()
    }

    /// Tree: prefix 한 레벨(소스 필터 반영) → 라벨/제목 부분일치 필터.
    fn recompute_tree(&self) -> Vec<ListItem> {
        let level = tree::build_tree_level(&self.rows, &self.prefix, &self.home, self.source_filter.as_str());
        tree::apply_tree_filter(level, &self.rows, &self.query)
    }

    fn current(&self) -> Option<&ListItem> {
        self.items.get(self.sel)
    }

    /// 현재 항목이 세션이면 그 행. (resume/copy/프리뷰/스코프 대상)
    fn current_session(&self) -> Option<&SessionRow> {
        match self.items.get(self.sel) {
            Some(ListItem::Session { row_idx }) => Some(&self.rows[*row_idx]),
            _ => None,
        }
    }

    /// 현재 선택(세션)+비빈 쿼리면 캐시된 base 프리뷰 라인에서 첫 매치로 앵커할 스크롤. 아니면 0.
    /// `recompute()`/`move_sel()` 의 스크롤 리셋을 대체. Ctrl+D/U 는 이걸 안 거치므로 수동 스크롤 우선. [D3]
    fn anchor_for_selection(&mut self) -> u16 {
        let terms = search::terms(&self.query);
        if terms.is_empty() {
            return 0;
        }
        let (path, src) = match self.items.get(self.sel) {
            Some(ListItem::Session { row_idx }) => {
                let r = &self.rows[*row_idx];
                let src = if r.source == "codex" {
                    Source::Codex
                } else {
                    Source::Claude
                };
                (r.path.clone(), src)
            }
            _ => return 0,
        };
        // base(하이라이트 없는) 라인 캐시에서 매치 위치 계산 — 검색 haystack 아니라 렌더 텍스트 기준. [D2]
        let lines = self
            .preview_cache
            .entry(path.clone())
            .or_insert_with(|| preview::render_session(&path, src));
        preview::anchor_scroll(lines, &terms, LEAD_IN)
    }

    /// Alt+n(dir=1)/Alt+p(dir=-1): 현재 세션 매치들에서 현 스크롤 기준 다음/이전 매치로 앵커(순환).
    /// 세션 아님·빈 쿼리·매치 0개면 no-op. [D8]
    fn jump_match(&mut self, dir: i32) {
        let terms = search::terms(&self.query);
        if terms.is_empty() {
            return;
        }
        let (path, src) = match self.items.get(self.sel) {
            Some(ListItem::Session { row_idx }) => {
                let r = &self.rows[*row_idx];
                let src = if r.source == "codex" {
                    Source::Codex
                } else {
                    Source::Claude
                };
                (r.path.clone(), src)
            }
            _ => return,
        };
        let lines = self
            .preview_cache
            .entry(path.clone())
            .or_insert_with(|| preview::render_session(&path, src));
        let matches = preview::match_lines(lines, &terms);
        if matches.is_empty() {
            return;
        }
        // 현재 앵커된 매치 위치 추정 = scroll + LEAD_IN. 이보다 다음/이전 매치로 이동(끝에서 순환).
        let cur = self.preview_scroll as usize + LEAD_IN as usize;
        let target = if dir > 0 {
            matches
                .iter()
                .copied()
                .find(|&m| m > cur)
                .unwrap_or(matches[0])
        } else {
            matches
                .iter()
                .rev()
                .copied()
                .find(|&m| m < cur)
                .unwrap_or_else(|| *matches.last().unwrap())
        };
        self.preview_scroll = (target.min(u16::MAX as usize) as u16).saturating_sub(LEAD_IN);
    }

    /// 순수 키 핸들러. 종료 액션이면 Some(Outcome). 새 ctrl 암은 `Char(c) if !ctrl` catch-all **앞**.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Outcome> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => return Some(Outcome::Quit),
            KeyCode::Char('c') if ctrl => return Some(Outcome::Quit),
            // 트리/스코프 (Tab 은 Char 아님 → catch-all 안 탐; ctrl-* 는 if ctrl 가드)
            KeyCode::Tab => self.enter_tree(),
            KeyCode::Char('o') if ctrl => self.enter_tree(),
            KeyCode::Char('h') if ctrl => self.tree_up(),
            KeyCode::Char('g') if ctrl => self.toggle_scope(),
            KeyCode::Char('s') if ctrl => self.cycle_source(),
            KeyCode::Char('a') if ctrl => self.reset_all(),
            KeyCode::Enter => return self.on_enter(),
            KeyCode::Char('y') if ctrl => {
                if let Some(r) = self.current_session() {
                    return Some(Outcome::Copy(r.clone()));
                }
            }
            KeyCode::Down => self.move_sel(1),
            KeyCode::Char('n') if ctrl => self.move_sel(1),
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('p') if ctrl => self.move_sel(-1),
            KeyCode::Char('d') if ctrl => self.preview_scroll = self.preview_scroll.saturating_add(10),
            KeyCode::Char('u') if ctrl => self.preview_scroll = self.preview_scroll.saturating_sub(10),
            // 매치 순회(P3). 평문 글자는 쿼리로 새므로 Alt 모디파이어 사용. [D8]
            KeyCode::Char('n') if alt => self.jump_match(1),
            KeyCode::Char('p') if alt => self.jump_match(-1),
            KeyCode::Backspace => {
                self.query.pop();
                self.recompute();
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.query.push(c);
                self.recompute();
            }
            _ => {}
        }
        None
    }

    /// tab/ctrl-o: 계층 트리 뷰(ROOT=HOME)로 진입. scope 는 **건드리지 않음**(Phase A 파리티 — ctrl-a 가 전체 리셋).
    fn enter_tree(&mut self) {
        self.view = ViewMode::Tree;
        self.prefix.clear();
        self.query.clear();
        self.sel = 0;
        self.recompute();
    }

    /// ctrl-h: 트리 한 단계 위로 (Flat 또는 ROOT 면 no-op).
    fn tree_up(&mut self) {
        if self.view != ViewMode::Tree || self.prefix.is_empty() {
            return;
        }
        let par = tree::parent_clamped(&self.prefix, &self.home);
        self.prefix = if par == self.home { String::new() } else { par };
        self.query.clear();
        self.sel = 0;
        self.recompute();
    }

    /// ctrl-g: 호버 세션의 프로젝트 스코프 토글(같으면 해제). Dir/UP/cwd없음이면 no-op. Flat 세션 뷰로 복귀.
    fn toggle_scope(&mut self) {
        let cwd = match self.current_session() {
            Some(r) => match &r.cwd {
                Some(c) => c.clone(),
                None => return,
            },
            None => return,
        };
        self.scope = if self.scope.as_deref() == Some(cwd.as_str()) {
            None
        } else {
            Some(cwd)
        };
        self.view = ViewMode::Flat;
        self.prefix.clear();
        self.query.clear();
        self.sel = 0;
        self.recompute();
    }

    /// ctrl-s: 소스 필터 순환(전체→Claude→Codex→전체). 뷰/스코프/쿼리는 유지, 리스트만 갱신.
    fn cycle_source(&mut self) {
        self.source_filter = self.source_filter.next();
        self.sel = 0;
        self.recompute();
    }

    /// ctrl-a: 전체(스코프/트리/쿼리/소스필터 해제).
    fn reset_all(&mut self) {
        self.scope = None;
        self.source_filter = SourceFilter::All;
        self.view = ViewMode::Flat;
        self.prefix.clear();
        self.query.clear();
        self.sel = 0;
        self.recompute();
    }

    /// 호버 항목의 강조색 — Session=소스색(Claude 주황/Codex 그린), 그 외(Dir/Up/없음)=중립.
    fn accent(&self) -> Color {
        match self.current() {
            Some(ListItem::Session { row_idx }) => theme::source_color(&self.rows[*row_idx].source),
            _ => theme::NEUTRAL,
        }
    }

    /// enter: Up/Dir 면 드릴(트리 유지), Session 이면 Resume. 빈 목록이면 no-op.
    fn on_enter(&mut self) -> Option<Outcome> {
        enum Act {
            Drill(String),
            Resume(usize),
            None,
        }
        // 먼저 불변 차용으로 액션만 추출(차용 종료 후 변이).
        let act = match self.current() {
            Some(ListItem::Up { parent }) => Act::Drill(parent.clone()),
            Some(ListItem::Dir { node, .. }) => Act::Drill(node.clone()),
            Some(ListItem::Session { row_idx }) => Act::Resume(*row_idx),
            None => Act::None,
        };
        match act {
            Act::Drill(target) => {
                self.prefix = if target == self.home { String::new() } else { target };
                self.view = ViewMode::Tree;
                self.query.clear();
                self.sel = 0;
                self.recompute();
                None
            }
            Act::Resume(idx) => Some(Outcome::Resume(self.rows[idx].clone())),
            Act::None => None,
        }
    }

    fn move_sel(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let max = self.items.len() - 1;
        let next = (self.sel as i32 + delta).clamp(0, max as i32) as usize;
        if next != self.sel {
            self.sel = next;
            self.preview_scroll = self.anchor_for_selection();
        }
    }

    /// 현재 선택 항목의 프리뷰 라인(캐시). Session=대화 렌더, Dir=리치 트리 요약, Up=힌트.
    fn preview_content(&mut self) -> Vec<Line<'static>> {
        enum Kind {
            Session(String, Source),
            Dir(String),
            Up,
            None,
        }
        let kind = match self.items.get(self.sel) {
            Some(ListItem::Session { row_idx }) => {
                let r = &self.rows[*row_idx];
                let src = if r.source == "codex" { Source::Codex } else { Source::Claude };
                Kind::Session(r.path.clone(), src)
            }
            Some(ListItem::Dir { node, .. }) => Kind::Dir(node.clone()),
            Some(ListItem::Up { .. }) => Kind::Up,
            None => Kind::None,
        };
        match kind {
            Kind::Session(path, src) => {
                let terms = search::terms(&self.query);
                let key = format!("{path}\u{0}{}", self.query);
                // 하이라이트 캐시 히트(스크롤 중 query 불변) → base 재클론 없이 반환. [D6]
                if !terms.is_empty() {
                    if let Some(hl) = self.hl_cache.get(&key) {
                        return hl.clone();
                    }
                }
                let base = self
                    .preview_cache
                    .entry(path.clone())
                    .or_insert_with(|| preview::render_session(&path, src))
                    .clone();
                if terms.is_empty() {
                    return base;
                }
                let hl = preview::highlight_lines(&base, &terms);
                self.hl_cache.insert(key, hl.clone());
                hl
            }
            Kind::Dir(node) => {
                if let Some(c) = self.preview_cache.get(&node) {
                    return c.clone();
                }
                let lines = dir_preview_lines(&self.rows, &node, &self.home, self.now);
                self.preview_cache.insert(node, lines.clone());
                lines
            }
            Kind::Up => vec![Line::from(Span::styled(
                "⬅  상위 디렉토리로 (enter)",
                Style::default().add_modifier(Modifier::DIM),
            ))],
            Kind::None => vec![Line::from("")],
        }
    }

    /// 헤더 카운트. Flat=필터된/전체 세션, Tree=현 레벨 디렉토리·세션 분리(전역 분모 혼동 방지).
    fn header_count(&self) -> String {
        match self.view {
            ViewMode::Flat => format!("{}/{}", self.items.len(), self.rows.len()),
            ViewMode::Tree => {
                let dirs = self.items.iter().filter(|i| matches!(i, ListItem::Dir { .. })).count();
                let sess = self.items.iter().filter(|i| i.is_session()).count();
                format!("{dirs}📁 {sess}세션")
            }
        }
    }

    /// 프롬프트 라벨 = (소스필터)·(위치). 예: `search>` · `claude>` · `~/work/회사>` · `codex·work/csess>`.
    fn prompt_label(&self) -> String {
        let loc: Option<String> = match self.view {
            ViewMode::Tree => {
                let p = if self.prefix.is_empty() {
                    self.home.as_str()
                } else {
                    self.prefix.as_str()
                };
                Some(tree::homerel(p, &self.home))
            }
            ViewMode::Flat => self.scope.as_deref().map(model::proj_label),
        };
        match (self.source_filter, loc) {
            (SourceFilter::All, None) => "search> ".to_string(),
            (SourceFilter::All, Some(l)) => format!("{l}> "),
            (sf, None) => format!("{}> ", sf.as_str().unwrap_or("")),
            (sf, Some(l)) => format!("{}·{l}> ", sf.as_str().unwrap_or("")),
        }
    }

    pub fn render(&mut self, f: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(f.area());

        // 검색/위치 바. 프롬프트색 = 호버 소스(accent).
        let accent = self.accent();
        let count = self.header_count();
        let hint = match self.view {
            ViewMode::Tree => "enter=drill/resume · ctrl-h=up · ctrl-s=source · ctrl-a=all · esc=quit",
            ViewMode::Flat => "enter=resume · tab=tree · ctrl-g=scope · ctrl-s=source · esc=quit",
        };
        let prompt = self.prompt_label();
        let header = Line::from(vec![
            Span::styled(prompt.clone(), Style::default().fg(accent)),
            Span::raw(self.query.clone()),
            Span::styled(
                format!("    [{count}]  {hint}"),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]);
        f.render_widget(Paragraph::new(header), chunks[0]);
        // 커서를 프롬프트+쿼리 끝 칸에 둔다 → 한글/CJK IME 의 조합중(preedit) 글자가 그 자리에
        // 인라인으로 보인다(안 그러면 커서 숨김 → 조합 완료돼야만 나타남). 표시폭으로 계산(CJK 2칸). [search-hit-preview]
        let cw = (prompt.width() + self.query.width()).min(u16::MAX as usize) as u16;
        let cx = chunks[0]
            .x
            .saturating_add(cw)
            .min(chunks[0].right().saturating_sub(1));
        f.set_cursor_position(Position::new(cx, chunks[0].y));

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(chunks[1]);

        // 리스트 (UP/Dir/Session 3종)
        let dim = Style::default().add_modifier(Modifier::DIM);
        let items: Vec<UiListItem> = self
            .items
            .iter()
            .map(|it| match it {
                ListItem::Up { .. } => {
                    UiListItem::new(Line::from(Span::styled("  ⬅  ..", dim)))
                }
                ListItem::Dir { label, count, max_mtime, .. } => {
                    let rt = model::reltime(*max_mtime, self.now);
                    UiListItem::new(Line::from(vec![
                        Span::styled(format!("{rt:>4} "), dim),
                        Span::styled(
                            format!("📁 {label}/"),
                            Style::default().fg(theme::NEUTRAL).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!("  {count}개"), dim),
                    ]))
                }
                ListItem::Session { row_idx } => {
                    let r = &self.rows[*row_idx];
                    let rt = model::reltime(r.mtime, self.now);
                    // Claude=주황 c / Codex=그린 ⓒ
                    let (tag, scolor) = if r.source == "codex" {
                        ("ⓒ", theme::CODEX)
                    } else {
                        ("c", theme::CLAUDE)
                    };
                    UiListItem::new(Line::from(vec![
                        Span::styled(format!("{rt:>4} "), dim),
                        Span::styled(format!("{tag} "), Style::default().fg(scolor)),
                        Span::raw(truncate(&r.title, 48)),
                        Span::styled(format!("  {}", r.proj), dim),
                    ]))
                }
            })
            .collect();
        let mut state = ListState::default();
        if !self.items.is_empty() {
            state.select(Some(self.sel));
        }
        let list = List::new(items)
            .block(Block::default().borders(Borders::RIGHT))
            .highlight_style(
                Style::default()
                    .fg(accent)
                    .bg(Color::Rgb(40, 44, 52))
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▌");
        f.render_stateful_widget(list, body[0], &mut state);

        // 프리뷰
        let title = match self.current() {
            Some(ListItem::Session { row_idx }) => {
                let r = &self.rows[*row_idx];
                format!(
                    " {} {} ",
                    if r.source == "codex" { "[codex]" } else { "[claude]" },
                    r.proj
                )
            }
            Some(ListItem::Dir { .. }) => " [tree] ".to_string(),
            _ => String::new(),
        };
        let lines = self.preview_content();
        let preview = Paragraph::new(lines)
            .block(Block::default().title(title))
            .scroll((self.preview_scroll, 0));
        f.render_widget(preview, body[1]);
    }
}

/// Dir 행 리치 프리뷰(Phase A `csess_preview_tree` 파리티) — 인메모리, 파일 I/O 없음.
/// 헤더(노드+총 세션수) + 하위 디렉토리(카운트, 최근활동=mtime desc 순) + 최근 세션 제목(최대 40, mtime desc).
fn dir_preview_lines(rows: &[SessionRow], node: &str, home: &str, now: i64) -> Vec<Line<'static>> {
    const MAX_SESS: usize = 40;
    let mut total = 0usize;
    let mut child_counts: HashMap<String, usize> = HashMap::new();
    let mut child_order: Vec<String> = Vec::new();
    let mut sessions: Vec<(String, String)> = Vec::new(); // (reltime, title)

    for r in rows {
        let cwd = match &r.cwd {
            Some(c) => c.as_str(),
            None => continue,
        };
        if !tree::under(cwd, node) {
            continue;
        }
        total += 1;
        if cwd != node {
            let seg = tree::relseg(cwd, node).to_string();
            if !child_counts.contains_key(&seg) {
                child_order.push(seg.clone());
            }
            *child_counts.entry(seg).or_insert(0) += 1;
        }
        if sessions.len() < MAX_SESS {
            sessions.push((model::reltime(r.mtime, now), r.title.clone()));
        }
    }
    // child_order 는 첫 등장 순서 = mtime desc(rows 가 mtime desc) — Phase A 프리뷰와 동일. 정렬 안 함.

    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(format!("📂 {}", tree::homerel(node, home)), bold),
        Span::styled(format!("   {total} 세션"), dim),
    ]));
    lines.push(Line::from(""));
    if !child_order.is_empty() {
        lines.push(Line::from(Span::styled("하위 디렉토리:", dim)));
        for seg in &child_order {
            lines.push(Line::from(vec![
                Span::styled(format!("  📁 {seg}/"), bold),
                Span::styled(format!(" ({})", child_counts[seg]), dim),
            ]));
        }
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled("최근 세션 (enter 로 진입/resume):", dim)));
    for (rt, title) in &sessions {
        lines.push(Line::from(vec![
            Span::styled(format!("  {rt:>4}  "), dim),
            Span::raw(title.clone()),
        ]));
    }
    lines
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
pub fn run(rows: Vec<SessionRow>, now: i64, home: String) -> Outcome {
    let mut term = ratatui::init();
    let mut app = App::new(rows, now, home);
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

    const HOME: &str = "/home/u";

    fn rows() -> Vec<SessionRow> {
        vec![
            mkrow("claude", "alpha auth jwt", "/home/u/work/a", "work/a", 300),
            mkrow("codex", "beta deploy", "/home/u/work/b", "work/b", 200),
            mkrow("claude", "gamma auth token", "/home/u/work/c", "work/c", 100),
        ]
    }
    fn mkrow(src: &str, title: &str, cwd: &str, proj: &str, mtime: i64) -> SessionRow {
        SessionRow {
            source: src.into(),
            id: "id".into(),
            cwd: Some(cwd.into()),
            cwd_lossy: false,
            path: "/no/such/path.jsonl".into(),
            mtime,
            title: title.into(),
            proj: proj.into(),
            body: String::new(),
        }
    }
    fn app() -> App {
        App::new(rows(), 1000, HOME.into())
    }
    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn alt(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
    }
    fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn typing_filters_and_resets() {
        let mut app = app();
        assert_eq!(app.items.len(), 3);
        for c in "auth".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.query, "auth");
        assert_eq!(app.items.len(), 2); // alpha, gamma
        for c in " jwt".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.items.len(), 1);
        for _ in 0..4 {
            app.handle_key(plain(KeyCode::Backspace));
        }
        assert_eq!(app.query, "auth");
        assert_eq!(app.items.len(), 2);
    }

    #[test]
    fn navigation_and_enter_outcome() {
        let mut app = app();
        app.handle_key(plain(KeyCode::Down));
        assert_eq!(app.sel, 1);
        app.handle_key(ctrl('n'));
        assert_eq!(app.sel, 2);
        app.handle_key(ctrl('n')); // clamp
        assert_eq!(app.sel, 2);
        app.handle_key(plain(KeyCode::Up));
        assert_eq!(app.sel, 1);
        match app.handle_key(plain(KeyCode::Enter)) {
            Some(Outcome::Resume(r)) => assert_eq!(r.title, "beta deploy"),
            _ => panic!("expected Resume"),
        }
        assert!(matches!(app.handle_key(ctrl('y')), Some(Outcome::Copy(_))));
        assert!(matches!(app.handle_key(plain(KeyCode::Esc)), Some(Outcome::Quit)));
    }

    #[test]
    fn tab_enters_tree_and_ctrl_a_resets() {
        let mut app = app();
        app.handle_key(plain(KeyCode::Tab));
        assert_eq!(app.view, ViewMode::Tree);
        assert!(app.prefix.is_empty());
        // ROOT: Dir work(3개) 한 줄 (work/a,b,c 가 work 로 압축 — 단일 자식 아님 → 'work' 노드, 직속 세션 없음)
        assert!(app.items.iter().any(|i| matches!(i, ListItem::Dir { .. })));
        // ctrl-o 도 트리 진입
        app.handle_key(ctrl('a'));
        assert_eq!(app.view, ViewMode::Flat);
        app.handle_key(ctrl('o'));
        assert_eq!(app.view, ViewMode::Tree);
    }

    #[test]
    fn enter_drills_then_resumes() {
        let mut app = app();
        app.handle_key(plain(KeyCode::Tab)); // 트리 ROOT
        // 첫 행 = Dir work → enter 로 드릴
        assert!(matches!(app.items[0], ListItem::Dir { .. }));
        app.handle_key(plain(KeyCode::Enter));
        assert_eq!(app.view, ViewMode::Tree);
        assert_eq!(app.prefix, "/home/u/work");
        // 이제 work 밑: UP + work/a,b,c 세 디렉토리 (각 1세션) — 직속 세션 없음
        assert!(matches!(app.items[0], ListItem::Up { .. }));
        // a/b/c 드릴 → 세션 → enter resume
        let dir_idx = app.items.iter().position(|i| matches!(i, ListItem::Dir { .. })).unwrap();
        app.sel = dir_idx;
        app.handle_key(plain(KeyCode::Enter));
        let sess_idx = app.items.iter().position(|i| i.is_session()).unwrap();
        app.sel = sess_idx;
        assert!(matches!(app.handle_key(plain(KeyCode::Enter)), Some(Outcome::Resume(_))));
    }

    #[test]
    fn ctrl_h_navigates_up_and_noops() {
        let mut app = app();
        // Flat 에서 ctrl-h no-op
        app.handle_key(ctrl('h'));
        assert_eq!(app.view, ViewMode::Flat);
        // 트리 ROOT 에서 ctrl-h no-op
        app.handle_key(plain(KeyCode::Tab));
        app.handle_key(ctrl('h'));
        assert!(app.prefix.is_empty());
        // 드릴 후 ctrl-h → ROOT 복귀
        app.prefix = "/home/u/work".into();
        app.recompute();
        app.handle_key(ctrl('h'));
        assert!(app.prefix.is_empty()); // 부모가 HOME → 빈 문자열
    }

    #[test]
    fn ctrl_g_toggles_scope() {
        let mut app = app();
        app.sel = 0; // alpha @ /home/u/work/a
        app.handle_key(ctrl('g'));
        assert_eq!(app.scope.as_deref(), Some("/home/u/work/a"));
        assert_eq!(app.items.len(), 1); // 그 cwd 세션만
        // 같은 세션에 다시 → 해제
        app.handle_key(ctrl('g'));
        assert_eq!(app.scope, None);
        assert_eq!(app.items.len(), 3);
    }

    #[test]
    fn ctrl_g_noop_on_dir() {
        let mut app = app();
        app.handle_key(plain(KeyCode::Tab)); // 트리, 첫 행 Dir
        app.sel = 0;
        assert!(matches!(app.items[0], ListItem::Dir { .. }));
        app.handle_key(ctrl('g'));
        assert_eq!(app.scope, None); // Dir 에선 no-op
    }

    #[test]
    fn ctrl_keys_dont_leak_into_query() {
        let mut app = app();
        for k in ['g', 'h', 'a', 'o', 's'] {
            app.handle_key(ctrl(k));
        }
        assert_eq!(app.query, ""); // 어느 것도 쿼리에 안 들어감
    }

    #[test]
    fn ctrl_s_cycles_source_filter() {
        let mut app = app(); // 2 claude + 1 codex
        assert_eq!(app.items.len(), 3);
        app.handle_key(ctrl('s')); // → Claude만
        assert_eq!(app.source_filter, SourceFilter::Claude);
        assert_eq!(app.items.len(), 2);
        app.handle_key(ctrl('s')); // → Codex만
        assert_eq!(app.source_filter, SourceFilter::Codex);
        assert_eq!(app.items.len(), 1);
        app.handle_key(ctrl('s')); // → 전체
        assert_eq!(app.source_filter, SourceFilter::All);
        assert_eq!(app.items.len(), 3);
        // ctrl-a 가 소스필터도 리셋
        app.handle_key(ctrl('s'));
        app.handle_key(ctrl('a'));
        assert_eq!(app.source_filter, SourceFilter::All);
    }

    #[test]
    fn prompt_label_reflects_source_and_loc() {
        let mut app = app();
        assert_eq!(app.prompt_label(), "search> ");
        app.handle_key(ctrl('s')); // Claude
        assert_eq!(app.prompt_label(), "claude> ");
        app.handle_key(plain(KeyCode::Tab)); // tree ROOT, Claude 유지
        assert_eq!(app.prompt_label(), "claude·~> ");
    }

    #[test]
    fn renders_flat_and_tree() {
        let backend = TestBackend::new(100, 20);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = app();
        term.draw(|f| app.render(f)).unwrap();
        let flat: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(flat.contains("search>"));
        assert!(flat.contains("alpha"));
        // 트리 진입 → 프롬프트 '~>' + 📁
        app.handle_key(plain(KeyCode::Tab));
        term.draw(|f| app.render(f)).unwrap();
        let tree: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(tree.contains("~>"));
        assert!(tree.contains("📁"));
    }

    #[test]
    fn tree_header_count_branches() {
        // Flat=필터/전체, Tree=디렉토리·세션 분리. private 포맷을 직접 검증(와이드문자 버퍼 회피).
        let mut app = app();
        let flat = app.header_count();
        assert_eq!(flat, "3/3");
        app.handle_key(plain(KeyCode::Tab));
        let tree = app.header_count();
        // ROOT: Dir work 1개 + 직속세션 0 → "1📁 0세션" (전역 분모 '/3' 아님)
        assert_eq!(tree, "1📁 0세션");
    }

    #[test]
    fn scope_prompt_label() {
        let mut app = app();
        app.sel = 0;
        app.handle_key(ctrl('g')); // scope = /home/u/work/a
        let backend = TestBackend::new(100, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| app.render(f)).unwrap();
        let s: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(s.contains("work/a>")); // proj_label 프롬프트
    }

    fn plain_line(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn anchor_jumps_to_body_match_and_manual_scroll_wins() {
        let mut app = app();
        // sel=0 세션의 base 프리뷰 라인을 캐시에 시드(파일 I/O 회피). "auth" 를 idx7 에 배치.
        let path = "/no/such/path.jsonl".to_string();
        let mut lines: Vec<Line> = (0..20).map(|i| Line::from(format!("filler {i}"))).collect();
        lines[7] = Line::from("여기 auth 매치");
        app.preview_cache.insert(path, lines);
        // "auth" 는 row0 title("alpha auth jwt")에도 있어 리스트에 남고, 캐시 본문 idx7 에도 있음
        for c in "auth".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.sel, 0);
        assert_eq!(app.preview_scroll, 5); // 7 - LEAD_IN(2)
        // Ctrl+D → 수동 스크롤 우선(재앵커 안 함)
        app.handle_key(ctrl('d'));
        assert_eq!(app.preview_scroll, 15); // 5 + 10
        // 쿼리 변경(backspace) → 재앵커
        app.handle_key(plain(KeyCode::Backspace)); // "aut", 여전히 idx7 매치
        assert_eq!(app.preview_scroll, 5);
    }

    #[test]
    fn no_anchor_on_empty_query_or_title_only_match() {
        let mut app = app();
        // 빈 쿼리 → 0
        assert_eq!(app.anchor_for_selection(), 0);
        // 제목만 매치(본문 캐시엔 없음) → top 폴백(0). [F3 / FR-006 / FR-007]
        let path = "/no/such/path.jsonl".to_string();
        app.preview_cache.insert(path, vec![Line::from("본문엔 매치 없음")]);
        for c in "auth".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.sel, 0); // "auth" 는 title 매치라 리스트 유지
        assert_eq!(app.preview_scroll, 0); // 본문 렌더엔 매치 없음 → 앵커 0
    }

    #[test]
    fn preview_highlights_match_and_caches_by_path_query() {
        let mut app = app();
        let path = "/no/such/path.jsonl".to_string();
        app.preview_cache.insert(path.clone(), vec![Line::from("auth 라인")]);
        for c in "auth".chars() {
            app.handle_key(key(c));
        }
        // 첫 preview_content → 하이라이트 적용 + (path,query) 캐시 채움
        let first = app.preview_content();
        assert!(first[0].spans.iter().any(|s| s.style.bg == theme::match_hl().bg));
        assert!(app.hl_cache.contains_key(&format!("{path}\u{0}auth")));
        // 두 번째(스크롤 상당) → 캐시 히트, 동일 결과
        let second = app.preview_content();
        assert_eq!(plain_line(&second[0]), plain_line(&first[0]));
    }

    #[test]
    fn alt_n_p_navigate_matches_and_dont_leak() {
        let mut app = app();
        let path = "/no/such/path.jsonl".to_string();
        // "auth" 를 idx 3, 8, 14 에 배치
        let mut lines: Vec<Line> = (0..20).map(|i| Line::from(format!("filler {i}"))).collect();
        for &i in &[3usize, 8, 14] {
            lines[i] = Line::from("auth 여기");
        }
        app.preview_cache.insert(path, lines);
        for c in "auth".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.preview_scroll, 1); // 첫 매치 3 - LEAD_IN(2)
        app.handle_key(alt('n'));
        assert_eq!(app.preview_scroll, 6); // 8 - 2
        app.handle_key(alt('n'));
        assert_eq!(app.preview_scroll, 12); // 14 - 2
        app.handle_key(alt('n'));
        assert_eq!(app.preview_scroll, 1); // 순환 → 첫 매치
        app.handle_key(alt('p'));
        assert_eq!(app.preview_scroll, 12); // 이전(순환) → 마지막 매치
        assert_eq!(app.query, "auth"); // Alt+n/p 는 쿼리에 안 샘
    }

    #[test]
    fn cursor_sits_at_query_end_for_ime() {
        let backend = TestBackend::new(100, 20);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = app();
        for c in "가나".chars() {
            app.handle_key(key(c));
        }
        term.draw(|f| app.render(f)).unwrap();
        // "search> "(8칸) + "가나"(CJK 2×2=4칸) → 커서 x=12, 헤더 행 y=0
        // → IME 조합중 글자가 이 자리에 인라인으로 표시됨.
        let pos = term.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (12, 0));
    }
}
