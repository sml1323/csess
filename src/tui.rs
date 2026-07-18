//! TUI — ratatui 이벤트 루프. 리스트 + 본문검색 + 계층 트리(tab)/스코프(ctrl-g) + 인프로세스 프리뷰 + enter→resume.
//!
//! 설계: 키 핸들링은 순수 함수 `App::handle_key`(터미널 없이 단위테스트 — 클립보드 등 부수효과는
//! `pending_copy` 로 이벤트 루프에 위임), 렌더는 `App::render`(TestBackend 로 검증).
//! 이벤트 루프(`run`)만 crossterm 에 의존. resume 는 터미널 복원 **후** main 이 수행. [D7/D14]
//!
//! 레이아웃: 헤더(프롬프트+쿼리 | 필터 필+카운트) / 본문(리스트|프리뷰) / 푸터(status 또는 컨텍스트 힌트).
//! 뷰 상태: `view`(Flat/Tree) · `prefix`(트리 드릴 위치, 빈=ROOT=HOME) · `scope`(프로젝트 한정).
//! 리스트 항목은 [`crate::tree::ListItem`](UP/Dir/Session) 혼합 — 트리 빌더는 `tree` 모듈(순수).
//!
//! 프리뷰 좌표계: `preview_cache`(논리 라인) → [`preview::wrap_lines`](표시폭 랩, `wrap_cache`)
//! → 하이라이트(`hl_cache`). 앵커/매치순회/카운터/스크롤은 전부 **랩된 라인 인덱스** 기준 —
//! `pv_width` 변경(리사이즈) 시 랩 파생 캐시 전부 무효화. [search-hit-preview]

use crate::index::SessionRow;
use crate::model::{self, Source};
use crate::tree::{self, ListItem, ViewMode};
use crate::{preview, resume, search, theme};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Clear, List, ListItem as UiListItem, ListState, Padding, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;
use std::collections::HashMap;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

/// TUI 종료 결과. main 이 터미널 복원 후 처리. (복사는 인앱 처리 — Copy 종료 없음)
pub enum Outcome {
    Quit,
    Resume(SessionRow),
}

/// 앵커 시 매치 줄 위로 함께 보여줄 맥락 줄 수. [D7]
const LEAD_IN: u16 = 2;

pub struct App {
    rows: Vec<SessionRow>,
    haystacks: Vec<String>,
    query: String,
    last_query: String, // 쿼리-파생 캐시(hl/match)의 세대 키 — 바뀌면 통째 무효화(무한 성장 방지)
    items: Vec<ListItem>, // 현재 보이는 행(Flat=세션만, Tree=UP/Dir/세션 혼합)
    sel: usize,           // items 인덱스
    preview_cache: HashMap<String, Vec<Line<'static>>>, // 논리(비랩) 라인 — path/node 키
    wrap_cache: HashMap<String, Vec<Line<'static>>>, // pv_width 로 랩된 라인. 앵커/렌더 좌표계.
    hl_cache: HashMap<String, Vec<Line<'static>>>, // (path,query) → 하이라이트(랩 후). 스크롤 idle 유지. [D6]
    match_cache: HashMap<String, Vec<usize>>, // (path,query) → 랩 라인 매치 인덱스 (카운터/순회)
    preview_scroll: u16,
    pv_width: u16,  // 프리뷰 텍스트 폭(패딩·스크롤바 차감) — 렌더가 갱신, 변경 시 랩 캐시 무효화
    pv_height: u16, // 프리뷰 뷰포트 높이 — 스크롤 클램프용
    page: u16,      // 리스트 페이지 높이 (PageUp/Down)
    now: i64,
    home: String,          // $HOME (트리 루트)
    view: ViewMode,
    prefix: String,        // 트리 드릴 prefix, 빈=ROOT(=home)
    scope: Option<String>, // Some(cwd)=flat 뷰를 그 프로젝트로 한정
    source_filter: SourceFilter, // 전체/Claude만/Codex만 (ctrl-s)
    status: Option<String>, // 1턴 피드백(복사됨/거부) — 다음 키에서 클리어, 푸터에 표시
    pending_copy: Option<String>, // ctrl-y 가 남긴 복사 명령 — 이벤트 루프가 클립보드 실행
    show_help: bool,       // F1/alt-h 키맵 오버레이 (아무 키나 닫기)
}

impl App {
    pub fn new(rows: Vec<SessionRow>, now: i64, home: String) -> Self {
        let haystacks = search::build_haystacks(&rows);
        let mut app = App {
            rows,
            haystacks,
            query: String::new(),
            last_query: String::new(),
            items: Vec::new(),
            sel: 0,
            preview_cache: HashMap::new(),
            wrap_cache: HashMap::new(),
            hl_cache: HashMap::new(),
            match_cache: HashMap::new(),
            preview_scroll: 0,
            pv_width: 0,
            pv_height: 0,
            page: 0,
            now,
            home,
            view: ViewMode::Flat,
            prefix: String::new(),
            scope: None,
            source_filter: SourceFilter::All,
            status: None,
            pending_copy: None,
            show_help: false,
        };
        app.recompute();
        app
    }

    /// `self.items` 단일 진입점. 뷰에 따라 분기 + sel 클램프 + 프리뷰 스크롤 리셋.
    fn recompute(&mut self) {
        // 쿼리 세대 교체 → 쿼리-파생 캐시 무효화(옛 (path,query) 키가 영구 잔류하는 성장 차단).
        if self.query != self.last_query {
            self.hl_cache.clear();
            self.match_cache.clear();
            self.last_query = self.query.clone();
        }
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

    /// 현재 선택 세션의 (path, source). 프리뷰 파생 캐시들의 공통 진입점.
    fn current_path_src(&self) -> Option<(String, Source)> {
        self.current_session().map(|r| {
            let src = if r.source == "codex" { Source::Codex } else { Source::Claude };
            (r.path.clone(), src)
        })
    }

    /// path 의 논리 라인(preview_cache)과 랩 라인(wrap_cache)을 보장. [search-hit-preview]
    fn ensure_wrapped(&mut self, path: &str, src: Source) {
        if !self.preview_cache.contains_key(path) {
            let rendered = preview::render_session(path, src);
            self.preview_cache.insert(path.to_string(), rendered);
        }
        if !self.wrap_cache.contains_key(path) {
            let wrapped = preview::wrap_lines(&self.preview_cache[path], self.pv_width);
            self.wrap_cache.insert(path.to_string(), wrapped);
        }
    }

    /// Dir 프리뷰 캐시 키 — 소스 필터 차원 포함(필터 바뀌면 다른 키 → 스테일 없음).
    fn dir_key(&self, node: &str) -> String {
        format!("{node}\u{0}dir\u{0}{}", self.source_filter.as_str().unwrap_or("all"))
    }

    /// 현재 선택 항목의 랩된 프리뷰 길이(캐시 히트 시만 — ctrl-d 클램프용, I/O 없음).
    fn preview_len(&self) -> Option<usize> {
        match self.items.get(self.sel) {
            Some(ListItem::Session { row_idx }) => {
                self.wrap_cache.get(&self.rows[*row_idx].path).map(Vec::len)
            }
            Some(ListItem::Dir { node, .. }) => self.wrap_cache.get(&self.dir_key(node)).map(Vec::len),
            _ => None,
        }
    }

    /// (path, query) 의 랩 라인 매치 인덱스 — 캐시 히트 시 재계산 없음(스크롤 idle 유지).
    fn cached_matches(&mut self, path: &str, src: Source, terms: &[String]) -> Vec<usize> {
        let key = format!("{path}\u{0}{}", self.query);
        if let Some(m) = self.match_cache.get(&key) {
            return m.clone();
        }
        self.ensure_wrapped(path, src);
        let m = preview::match_lines(&self.wrap_cache[path], terms);
        if self.match_cache.len() >= 256 {
            self.match_cache.clear(); // 상한 백스톱(값은 작지만 무한 성장 차단)
        }
        self.match_cache.insert(key, m.clone());
        m
    }

    /// 현재 선택(세션)+비빈 쿼리면 랩된 프리뷰 라인에서 첫 매치로 앵커할 스크롤. 아니면 0.
    /// `recompute()`/`move_sel()` 의 스크롤 리셋을 대체. Ctrl+D/U 는 이걸 안 거치므로 수동 스크롤 우선. [D3]
    fn anchor_for_selection(&mut self) -> u16 {
        let terms = search::terms(&self.query);
        if terms.is_empty() {
            return 0;
        }
        let Some((path, src)) = self.current_path_src() else {
            return 0;
        };
        self.ensure_wrapped(&path, src);
        preview::anchor_scroll(&self.wrap_cache[&path], &terms, LEAD_IN)
    }

    /// Alt+n(dir=1)/Alt+p(dir=-1)/F3: 현재 세션 매치들에서 현 스크롤 기준 다음/이전 매치로 앵커(순환).
    /// 세션 아님·빈 쿼리·매치 0개면 no-op. [D8]
    fn jump_match(&mut self, dir: i32) {
        let terms = search::terms(&self.query);
        if terms.is_empty() {
            return;
        }
        let Some((path, src)) = self.current_path_src() else {
            return;
        };
        let matches = self.cached_matches(&path, src, &terms);
        if matches.is_empty() {
            return;
        }
        // 현재 매치 = 앵커(논리 scroll + LEAD_IN) 이하의 마지막 매치(current_match 와 동일 좌표계).
        // 서수 기준 ±1 순환 — scroll 역산(find m > cur)은 렌더 클램프·상단 saturating 과 얽혀
        // 끝/앞머리 매치에서 고착됐다(리뷰 확정 결함). preview_scroll 은 논리값(클램프는 표시 전용).
        let cur = self.preview_scroll as usize + LEAD_IN as usize;
        let n = matches.len();
        let c_idx = matches.partition_point(|&m| m <= cur); // cur 이하 매치 수 (0=아직 매치 전)
        let target = if dir > 0 {
            if c_idx >= n { matches[0] } else { matches[c_idx] }
        } else if c_idx >= 2 {
            matches[c_idx - 2]
        } else {
            matches[n - 1] // 첫 매치(또는 매치 전)에서 이전 → 마지막으로 순환
        };
        self.preview_scroll = (target.min(u16::MAX as usize) as u16).saturating_sub(LEAD_IN);
    }

    /// resume/복사 가능성 순수 검사(파일시스템 무접촉 — is_dir 최종 가드는 main 의 resume 가 수행).
    fn resumable(r: &SessionRow) -> Result<&str, String> {
        if r.id.is_empty() {
            return Err("⚠ 세션 id 없음 — resume 불가".to_string());
        }
        match &r.cwd {
            Some(c) if !c.is_empty() && !r.cwd_lossy => Ok(c.as_str()),
            _ => Err("⚠ cwd 불명 — resume 불가 (읽기 전용 세션)".to_string()),
        }
    }

    /// 순수 키 핸들러. 종료 액션이면 Some(Outcome). 새 ctrl 암은 `Char(c) if !ctrl` catch-all **앞**.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Outcome> {
        // 도움말 오버레이는 모달 — 아무 키나 닫고 키를 소비(쿼리로 새지 않음, esc 도 닫기만).
        if self.show_help {
            self.show_help = false;
            return None;
        }
        self.status = None; // 1턴 피드백 클리어
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => return Some(Outcome::Quit),
            KeyCode::Char('c') if ctrl => return Some(Outcome::Quit),
            KeyCode::F(1) => self.show_help = true,
            KeyCode::Char('h') if alt => self.show_help = true,
            // 트리/스코프 (Tab 은 Char 아님 → catch-all 안 탐; ctrl-* 는 if ctrl 가드)
            KeyCode::Tab => self.enter_tree(),
            KeyCode::Char('o') if ctrl => self.enter_tree(),
            KeyCode::Char('h') if ctrl => self.tree_up(),
            KeyCode::Char('g') if ctrl => self.toggle_scope(),
            KeyCode::Char('s') if ctrl => self.cycle_source(),
            KeyCode::Char('a') if ctrl => self.reset_all(),
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Enter => return self.on_enter(),
            KeyCode::Char('y') if ctrl => self.copy_current(),
            KeyCode::Down => self.move_sel(1),
            KeyCode::Char('n') if ctrl => self.move_sel(1),
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('p') if ctrl => self.move_sel(-1),
            KeyCode::PageDown => self.move_sel(self.page.max(1) as i32),
            KeyCode::PageUp => self.move_sel(-(self.page.max(1) as i32)),
            KeyCode::Home => self.jump_sel(0),
            KeyCode::End => self.jump_sel(self.items.len().saturating_sub(1)),
            KeyCode::Char('d') if ctrl => {
                // 콘텐츠 끝(마지막 페이지)에서 클램프 — 과주행으로 빈 화면에 갇히지 않게.
                // 랩 캐시 미충전(첫 렌더 전)이면 클램프 없이 진행(다음 렌더가 표시만 클램프).
                let next = self.preview_scroll.saturating_add(10);
                self.preview_scroll = match self.preview_len() {
                    Some(len) => {
                        let max = len.saturating_sub(self.pv_height as usize).min(u16::MAX as usize) as u16;
                        next.min(max)
                    }
                    None => next,
                };
            }
            KeyCode::Char('u') if ctrl => self.preview_scroll = self.preview_scroll.saturating_sub(10),
            // 매치 순회(P3). 평문 글자는 쿼리로 새므로 Alt 모디파이어(+F3 별칭 — macOS Option 비-Meta 우회). [D8]
            KeyCode::Char('n') if alt => self.jump_match(1),
            KeyCode::Char('p') if alt => self.jump_match(-1),
            KeyCode::F(3) => self.jump_match(1),
            KeyCode::Backspace if alt => self.delete_word(),
            KeyCode::Backspace => {
                if self.query.is_empty() {
                    // 빈 쿼리: 트리에선 상위로(파일매니저 관례), Flat 은 no-op(무의미한 재앵커 방지).
                    if self.view == ViewMode::Tree {
                        self.tree_up();
                    }
                } else {
                    self.query.pop();
                    self.recompute();
                }
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.query.push(c);
                self.recompute();
            }
            _ => {}
        }
        None
    }

    /// ctrl-w / alt-backspace: 쿼리 마지막 단어(+뒤따르는 공백) 삭제.
    fn delete_word(&mut self) {
        if self.query.is_empty() {
            return;
        }
        let t_len = self.query.trim_end_matches(' ').len();
        let cut = self.query[..t_len].rfind(' ').map(|i| i + 1).unwrap_or(0);
        self.query.truncate(cut);
        self.recompute();
    }

    /// ctrl-y: resume 명령을 pending_copy 로(클립보드 실행은 이벤트 루프 — 핸들러 순수성 유지).
    fn copy_current(&mut self) {
        let Some(r) = self.current_session() else {
            return;
        };
        match Self::resumable(r) {
            Ok(cwd) => self.pending_copy = Some(resume::command_string(r, cwd)),
            Err(e) => self.status = Some(e),
        }
    }

    /// tab/ctrl-o: 계층 트리 뷰(ROOT=HOME)로 진입. scope 는 **건드리지 않음**(Phase A 파리티 — ctrl-a 가 전체 리셋).
    fn enter_tree(&mut self) {
        self.view = ViewMode::Tree;
        self.prefix.clear();
        self.query.clear();
        self.sel = 0;
        self.recompute();
    }

    /// ctrl-h(트리)/빈 쿼리 backspace: 트리 한 단계 위로 (Flat 또는 ROOT 면 no-op).
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
    /// 쿼리는 **유지** — 스코프는 검색을 좁히는 조작이라 ctrl-s(소스필터)와 일관되게 검색어를 살린다.
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

    /// enter: Up/Dir 면 드릴(트리 유지), Session 이면 사전 검사 후 Resume — 거부는 status 로
    /// (TUI 를 죽이지 않음: 검색어/선택/필터 상태 보존). 빈 목록이면 no-op.
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
            Act::Resume(idx) => match Self::resumable(&self.rows[idx]) {
                Ok(_) => Some(Outcome::Resume(self.rows[idx].clone())),
                Err(e) => {
                    self.status = Some(format!("{e} · ctrl-y=복사도 불가 · 다른 세션을 선택"));
                    None
                }
            },
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

    /// Home/End: 지정 인덱스로 점프 + 재앵커.
    fn jump_sel(&mut self, to: usize) {
        if self.items.is_empty() || to == self.sel {
            return;
        }
        self.sel = to.min(self.items.len() - 1);
        self.preview_scroll = self.anchor_for_selection();
    }

    /// 현재 선택 항목의 프리뷰 라인(랩+하이라이트, 캐시). Session=대화 렌더, Dir=리치 트리 요약, Up=힌트.
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
                self.ensure_wrapped(&path, src);
                let base = self.wrap_cache[&path].clone();
                if terms.is_empty() {
                    return base;
                }
                let hl = preview::highlight_lines(&base, &terms);
                // 상한 백스톱: 쿼리 세대 무효화(recompute)가 1차 방어지만, 한 쿼리로 수백 세션을
                // 훑는 경우도 풀-카피 누적을 막는다.
                if self.hl_cache.len() >= 64 {
                    self.hl_cache.clear();
                }
                self.hl_cache.insert(key, hl.clone());
                hl
            }
            Kind::Dir(node) => {
                let key = self.dir_key(&node);
                if !self.preview_cache.contains_key(&key) {
                    let lines = dir_preview_lines(
                        &self.rows,
                        &node,
                        &self.home,
                        self.now,
                        self.source_filter.as_str(),
                    );
                    self.preview_cache.insert(key.clone(), lines);
                }
                if !self.wrap_cache.contains_key(&key) {
                    let wrapped = preview::wrap_lines(&self.preview_cache[&key], self.pv_width);
                    self.wrap_cache.insert(key.clone(), wrapped);
                }
                self.wrap_cache[&key].clone()
            }
            Kind::Up => vec![Line::from(Span::styled(
                ".. 상위 디렉토리로 (enter)",
                theme::dim(),
            ))],
            Kind::None => self.empty_state_lines(),
        }
    }

    /// 검색/필터 결과 0건일 때 리스트·프리뷰 공용 안내 — 원인(활성 필터)과 탈출 경로를 보여준다.
    fn empty_state_lines(&self) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = vec![Line::from("")];
        lines.push(Line::styled("  매치 없음".to_string(), Style::default().fg(theme::NEUTRAL)));
        lines.push(Line::from(""));
        if !self.query.is_empty() {
            lines.push(Line::styled(format!("  쿼리: {} (공백=AND)", self.query), theme::dim()));
        }
        if let Some(sf) = self.source_filter.as_str() {
            lines.push(Line::styled(format!("  소스 필터: {sf} (ctrl-s 순환)"), theme::dim()));
        }
        if let Some(sc) = &self.scope {
            lines.push(Line::styled(
                format!("  스코프: {} (ctrl-g 해제)", model::proj_label(sc)),
                theme::dim(),
            ));
        }
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "  backspace=쿼리 수정 · ctrl-a=전체 리셋".to_string(),
            theme::dim(),
        ));
        lines
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

    /// 프롬프트 라벨 — Flat 은 항상 `search>`(소스/스코프는 헤더 우측 필로 분리), Tree 는 현 위치.
    fn prompt_label(&self) -> String {
        match self.view {
            ViewMode::Tree => {
                let p = if self.prefix.is_empty() {
                    self.home.as_str()
                } else {
                    self.prefix.as_str()
                };
                format!("{}> ", tree::homerel(p, &self.home))
            }
            ViewMode::Flat => "search> ".to_string(),
        }
    }

    /// 푸터 컨텍스트 힌트 — 상태(뷰/쿼리/필터)에 따라 지금 의미 있는 키만.
    fn hints(&self) -> String {
        let mut h: Vec<&str> = Vec::new();
        match self.view {
            ViewMode::Tree => {
                h.push("enter=드릴/resume");
                h.push("bksp=상위");
            }
            ViewMode::Flat => {
                h.push("enter=resume");
                h.push("tab=트리");
            }
        }
        h.push("ctrl-g=스코프");
        h.push("ctrl-s=소스");
        h.push("ctrl-y=복사");
        if !self.query.is_empty() {
            h.push("M-n/p=매치");
        }
        h.push("ctrl-d/u=스크롤");
        if self.scope.is_some() || self.source_filter != SourceFilter::All || !self.query.is_empty() {
            h.push("ctrl-a=리셋");
        }
        h.push("F1=키맵");
        h.push("esc=종료");
        h.join(" · ")
    }

    /// 헤더 우측: 활성 스코프 필(pill) + 카운트. (소스 필터는 리스트 보더의 탭이 담당.)
    fn header_right(&self) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if let Some(sc) = &self.scope {
            spans.push(Span::styled(
                format!(" {} ", model::proj_label(sc)),
                Style::default().fg(Color::Black).bg(theme::NEUTRAL),
            ));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("{} ", self.header_count()),
            Style::default().fg(theme::NEUTRAL),
        ));
        Line::from(spans)
    }

    /// 리스트 보더 타이틀 — lazygit 식 소스 필터 탭. 활성 탭=소스색(전체=NEUTRAL) bold+밑줄,
    /// 비활성=dim. ctrl-s 순환과 1:1.
    fn source_tabs(&self) -> Line<'static> {
        let tabs = [
            ("all", SourceFilter::All, theme::NEUTRAL),
            ("claude", SourceFilter::Claude, theme::CLAUDE),
            ("codex", SourceFilter::Codex, theme::CODEX),
        ];
        // 보더 위에 얹히는 타이틀은 border_style(fg)을 상속하므로 fg 를 항상 명시 —
        // 안 그러면 비활성 탭이 보더색(호버 소스색)으로 물들어 활성처럼 보인다.
        let inactive = Style::default().fg(theme::NEUTRAL).add_modifier(Modifier::DIM);
        let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
        for (i, (label, sf, color)) in tabs.into_iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" · ", inactive));
            }
            if sf == self.source_filter {
                spans.push(Span::styled(
                    label.to_string(),
                    Style::default()
                        .fg(color)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ));
            } else {
                spans.push(Span::styled(label.to_string(), inactive));
            }
        }
        spans.push(Span::raw(" "));
        Line::from(spans)
    }

    /// 프리뷰 매치 카운터 (k, N) — 쿼리 활성 + 세션 선택일 때만. k 는 jump_match 와 동일 기준.
    fn preview_match_counter(&mut self) -> Option<(usize, usize)> {
        let terms = search::terms(&self.query);
        if terms.is_empty() {
            return None;
        }
        let (path, src) = self.current_path_src()?;
        let m = self.cached_matches(&path, src, &terms);
        let k = preview::current_match(&m, self.preview_scroll as usize + LEAD_IN as usize);
        Some((k, m.len()))
    }

    pub fn render(&mut self, f: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .split(f.area());

        // ── 헤더: 좌 프롬프트+쿼리(+플레이스홀더), 우 필터 필+카운트. 프롬프트색=호버 소스(accent).
        let accent = self.accent();
        let right = self.header_right();
        let rw = (right.width() as u16).min(chunks[0].width);
        let hdr = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(rw)])
            .split(chunks[0]);
        let prompt = self.prompt_label();
        let mut left_spans = vec![
            Span::styled(prompt.clone(), Style::default().fg(accent).add_modifier(Modifier::BOLD)),
            Span::raw(self.query.clone()),
        ];
        if self.query.is_empty() {
            let ph = match self.view {
                ViewMode::Flat => "본문 검색… (공백=AND)",
                ViewMode::Tree => "이 레벨 필터…",
            };
            left_spans.push(Span::styled(ph, theme::dim()));
        }
        f.render_widget(Paragraph::new(Line::from(left_spans)), hdr[0]);
        f.render_widget(Paragraph::new(right), hdr[1]);
        // 커서를 프롬프트+쿼리 끝 칸에 둔다 → 한글/CJK IME 의 조합중(preedit) 글자가 그 자리에
        // 인라인으로 보인다(안 그러면 커서 숨김 → 조합 완료돼야만 나타남). 표시폭으로 계산(CJK 2칸). [search-hit-preview]
        let cw = (prompt.width() + self.query.width()).min(u16::MAX as usize) as u16;
        let cx = hdr[0]
            .x
            .saturating_add(cw)
            .min(hdr[0].right().saturating_sub(1));
        f.set_cursor_position(Position::new(cx, chunks[0].y));

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(40), Constraint::Percentage(55)])
            .split(chunks[1]);
        self.page = body[0].height.saturating_sub(2); // 상하 보더

        // ── 리스트 (UP/Dir/Session 3종) — lazygit 식 보더 패널: 보더색=호버 소스(듀얼톤 크롬),
        // 타이틀=소스 필터 탭, 하단 우측=커서 위치 "k of N". 빈 결과는 안내 placeholder.
        let mut list_block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(accent))
            .title(self.source_tabs());
        if !self.items.is_empty() {
            list_block = list_block.title_bottom(
                Line::from(Span::styled(
                    format!(" {} of {} ", self.sel + 1, self.items.len()),
                    // 보더 타이틀은 border_style(accent)을 상속 — fg 명시로 차단(다른 타이틀과 동일 규칙).
                    Style::default().fg(theme::NEUTRAL).add_modifier(Modifier::DIM),
                ))
                .right_aligned(),
            );
        }
        if self.items.is_empty() {
            let ph = Paragraph::new(self.empty_state_lines()).block(list_block);
            f.render_widget(ph, body[0]);
        } else {
            let lw = body[0].width.saturating_sub(2) as usize; // 좌우 보더
            let terms = search::terms(&self.query);
            let items: Vec<UiListItem> = self
                .items
                .iter()
                .enumerate()
                .map(|(i, it)| {
                    let selected = i == self.sel;
                    let mut spans = self.row_spans(it, selected, lw, &terms);
                    if selected {
                        for s in spans.iter_mut() {
                            s.style = s
                                .style
                                .patch(Style::default().bg(theme::SEL_BG).add_modifier(Modifier::BOLD));
                        }
                    }
                    UiListItem::new(Line::from(spans))
                })
                .collect();
            let mut state = ListState::default();
            state.select(Some(self.sel));
            let list = List::new(items).block(list_block);
            f.render_stateful_widget(list, body[0], &mut state);
        }

        // ── 프리뷰: 폭 변경 감지 → 랩 파생 캐시 무효화 + 재앵커.
        let pv_area = body[1];
        let inner_w = pv_area.width.saturating_sub(4); // 좌우 보더 2 + 패딩 2 (스크롤바는 보더 위)
        if inner_w != self.pv_width {
            self.pv_width = inner_w;
            self.wrap_cache.clear();
            self.hl_cache.clear();
            self.match_cache.clear();
            self.preview_scroll = self.anchor_for_selection();
        }
        let lines = self.preview_content();
        let n_lines = lines.len();
        let vh = pv_area.height.saturating_sub(2); // 상하 보더
        self.pv_height = vh;
        // 표시 전용 클램프 — self.preview_scroll(논리 앵커, 매치 순회/카운터 기준)에 되쓰지 않는다.
        // 되쓰면 콘텐츠 끝 뷰포트 안 매치에서 alt-n 순환이 고착된다(리뷰 확정 결함).
        let max_scroll = n_lines.saturating_sub(vh as usize).min(u16::MAX as usize) as u16;
        let display_scroll = self.preview_scroll.min(max_scroll);

        // 보더 타이틀: 소스색 + proj + reltime, 우측 상단=매치 k/N, 우측 하단=스크롤 %.
        let counter = self.preview_match_counter();
        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::SEPARATOR))
            .padding(Padding::horizontal(1))
            .title(self.preview_title());
        // 보더 타이틀 스팬은 border_style(SEPARATOR)을 상속하므로 fg 명시(매몰 방지).
        let meta = Style::default().fg(theme::NEUTRAL).add_modifier(Modifier::DIM);
        if let Some((k, n)) = counter {
            let txt = if n == 0 {
                " 본문 매치 없음 ".to_string()
            } else {
                format!(" 매치 {k}/{n} · M-n/p ")
            };
            block = block.title_top(Line::from(Span::styled(txt, meta)).right_aligned());
        }
        if n_lines > vh as usize {
            let pct = display_scroll as usize * 100 / n_lines.saturating_sub(vh as usize).max(1);
            block = block.title_bottom(
                Line::from(Span::styled(format!(" {pct}% "), meta)).right_aligned(),
            );
        }
        let para = Paragraph::new(lines).block(block).scroll((display_scroll, 0));
        f.render_widget(para, pv_area);
        if n_lines > vh as usize {
            let mut sb = ScrollbarState::new(n_lines.saturating_sub(vh as usize))
                .position(display_scroll as usize);
            // 스크롤바는 우측 보더 위에 오버레이(lazygit 식) — 상하 모서리는 제외.
            let sb_area = Rect {
                y: pv_area.y + 1,
                height: pv_area.height.saturating_sub(2),
                ..pv_area
            };
            f.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None)
                    .track_style(Style::default().fg(theme::SEPARATOR))
                    .thumb_style(Style::default().fg(theme::NEUTRAL)),
                sb_area,
                &mut sb,
            );
        }

        // ── 푸터: status(1턴 피드백) 우선, 아니면 컨텍스트 힌트.
        let footer = match &self.status {
            Some(st) => {
                let color = if st.starts_with('⚠') { theme::WARN } else { theme::CODEX };
                Line::from(Span::styled(format!(" {st}"), Style::default().fg(color)))
            }
            None => Line::from(Span::styled(format!(" {}", self.hints()), theme::dim())),
        };
        f.render_widget(Paragraph::new(footer), chunks[2]);

        // ── 도움말 오버레이 (F1/alt-h, 아무 키나 닫기)
        if self.show_help {
            render_help(f, chunks[1]);
        }
    }

    /// 프리뷰 좌측 타이틀 — 소스명(소스색 bold) · proj(밝음) · reltime(dim).
    fn preview_title(&self) -> Line<'static> {
        match self.current() {
            Some(ListItem::Session { row_idx }) => {
                let r = &self.rows[*row_idx];
                let sc = theme::source_color(&r.source);
                // 보더 타이틀은 border_style(SEPARATOR)을 상속 — dim 스팬도 fg 명시(안 하면 매몰).
                let meta = Style::default().fg(theme::NEUTRAL).add_modifier(Modifier::DIM);
                Line::from(vec![
                    Span::styled(" ▍".to_string(), Style::default().fg(sc)),
                    Span::styled(r.source.clone(), Style::default().fg(sc).add_modifier(Modifier::BOLD)),
                    Span::styled(" · ".to_string(), meta),
                    Span::styled(r.proj.clone(), Style::default().fg(theme::TEXT_BRIGHT)),
                    Span::styled(format!(" · {} ", model::reltime(r.mtime, self.now)), meta),
                ])
            }
            Some(ListItem::Dir { .. }) => Line::from(Span::styled(
                " ▍tree ".to_string(),
                Style::default().fg(theme::NEUTRAL),
            )),
            _ => Line::from(""),
        }
    }

    /// 리스트 한 행의 스팬 — 표시폭 예산: 마커(1)+rt(5)+태그(2)+제목(+매치 강조)+패딩+우측 컬럼.
    fn row_spans(&self, it: &ListItem, selected: bool, lw: usize, terms: &[String]) -> Vec<Span<'static>> {
        let dim = theme::dim();
        let marker = if selected {
            Span::styled("▌".to_string(), Style::default().fg(self.accent()))
        } else {
            Span::raw(" ".to_string())
        };
        match it {
            ListItem::Up { .. } => {
                vec![marker, Span::raw("     ".to_string()), Span::styled("..".to_string(), dim)]
            }
            ListItem::Dir { label, count, max_mtime, .. } => {
                let rt = model::reltime(*max_mtime, self.now);
                let right = format!("{count}개");
                let fixed = 1 + 5 + 3; // 마커 + rt + "📁 "(이모지 2셀 + 공백)
                let right_w = right.width().min(lw / 3);
                let label_avail = lw.saturating_sub(fixed + right_w + 2);
                let label_t = format!("{}/", truncate_width(label, label_avail.saturating_sub(1)));
                let pad = lw.saturating_sub(fixed + label_t.width() + right_w);
                vec![
                    marker,
                    Span::styled(format!("{rt:>4} "), dim),
                    Span::styled(
                        format!("📁 {label_t}"),
                        Style::default().fg(theme::NEUTRAL).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(right, dim),
                ]
            }
            ListItem::Session { row_idx } => {
                let r = &self.rows[*row_idx];
                let rt = model::reltime(r.mtime, self.now);
                let bad = Self::resumable(r).is_err();
                // 소스 태그 — 폭 1 확정 ASCII (ⓒ 는 EAW ambiguous 로 열이 흔들렸다): c=Claude / x=Codex.
                let (tag, scolor) = if r.source == "codex" {
                    ("x", theme::CODEX)
                } else {
                    ("c", theme::CLAUDE)
                };
                let tag_color = if bad { theme::WARN } else { scolor };
                // 본문-only 매치 마커: terms 가 제목(truncation 전)에 하나도 없으면 표시.
                let title_lc = r.title.to_lowercase();
                let body_only = !terms.is_empty() && !terms.iter().any(|t| title_lc.contains(t.as_str()));
                let marker_txt = if body_only { "·본문" } else { "" };
                let fixed = 1 + 5 + 2; // 마커 + rt + 태그
                let mut right_w = r.proj.width().min(lw / 3);
                let mut title_avail = lw.saturating_sub(fixed + right_w + 2 + marker_txt.width());
                if title_avail < 8 {
                    right_w = 0;
                    title_avail = lw.saturating_sub(fixed + marker_txt.width());
                }
                let title_t = truncate_width(&r.title, title_avail);
                let base = if bad { dim } else { Style::default() };
                let mut spans = vec![
                    marker,
                    Span::styled(format!("{rt:>4} "), dim),
                    Span::styled(format!("{tag} "), Style::default().fg(tag_color)),
                ];
                spans.extend(match_spans(&title_t, terms, base));
                if body_only {
                    spans.push(Span::styled(marker_txt.to_string(), dim));
                }
                if right_w > 0 {
                    let proj_t = truncate_width(&r.proj, right_w);
                    let used = fixed + title_t.width() + marker_txt.width() + proj_t.width();
                    spans.push(Span::raw(" ".repeat(lw.saturating_sub(used))));
                    spans.push(Span::styled(proj_t, dim));
                }
                spans
            }
        }
    }
}

/// 제목 문자열을 검색어 매치 강조 스팬으로 분할 — 매치는 WARN 노랑 fg + BOLD(배경 없는 가벼운 변형,
/// 프리뷰 하이라이트와 같은 색조). terms 비면 통짜 스팬.
fn match_spans(text: &str, terms: &[String], base: Style) -> Vec<Span<'static>> {
    let ranges = preview::match_ranges(text, terms);
    if ranges.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let hl = Style::default().fg(theme::WARN).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut cur = 0usize;
    for (s, e) in ranges {
        if s > cur {
            spans.push(Span::styled(text[cur..s].to_string(), base));
        }
        spans.push(Span::styled(text[s..e].to_string(), base.patch(hl)));
        cur = e;
    }
    if cur < text.len() {
        spans.push(Span::styled(text[cur..].to_string(), base));
    }
    spans
}

/// 도움말 오버레이 — 전체 키맵. 아무 키나 닫기.
fn render_help(f: &mut Frame, within: Rect) {
    let rows: &[(&str, &str)] = &[
        ("타이핑", "본문 검색 (공백=AND, 대소문자 무시)"),
        ("↑↓ / ctrl-p,n", "이동 · PgUp/PgDn=페이지 · Home/End=처음/끝"),
        ("enter", "resume (트리: 디렉토리 드릴)"),
        ("tab / ctrl-o", "트리 뷰 (ROOT=~)"),
        ("ctrl-h / bksp", "트리 한 단계 위 (bksp 는 빈 쿼리일 때)"),
        ("ctrl-g", "호버 세션의 프로젝트 스코프 토글"),
        ("ctrl-s", "소스 필터 (전체→claude→codex)"),
        ("ctrl-a", "쿼리/스코프/소스/트리 전체 리셋"),
        ("ctrl-y", "resume 명령 클립보드 복사 (앱 유지)"),
        ("ctrl-d / ctrl-u", "프리뷰 스크롤 ±10줄"),
        ("alt-n / alt-p", "다음/이전 매치로 점프 (F3=다음)"),
        ("ctrl-w / alt-bksp", "쿼리 마지막 단어 삭제"),
        ("F1 / alt-h", "이 도움말"),
        ("esc / ctrl-c", "종료"),
    ];
    let w = 64.min(within.width);
    let h = (rows.len() as u16 + 4).min(within.height);
    let area = Rect {
        x: within.x + (within.width.saturating_sub(w)) / 2,
        y: within.y + (within.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, area);
    let mut lines: Vec<Line> = vec![Line::from("")];
    for (k, v) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<18}"), Style::default().fg(theme::WARN)),
            Span::raw((*v).to_string()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  아무 키나 눌러 닫기", theme::dim())));
    let help = Paragraph::new(lines).block(
        Block::bordered()
            .border_type(BorderType::Rounded)
            .title(Line::from(Span::styled(
                " 키맵 ",
                Style::default().fg(theme::NEUTRAL).add_modifier(Modifier::BOLD),
            )))
            .border_style(Style::default().fg(theme::SEPARATOR)),
    );
    f.render_widget(help, area);
}

/// Dir 행 리치 프리뷰(Phase A `csess_preview_tree` 파리티) — 인메모리, 파일 I/O 없음.
/// 헤더(노드+총 세션수) + 하위 디렉토리(카운트, 최근활동=mtime desc 순) + 최근 세션(소스 태그 포함,
/// 최대 40 + 초과분 표시). `source` 필터는 리스트(build_tree_level)와 동일 적용 — 리스트 카운트와
/// 프리뷰 카운트가 모순되지 않게(리뷰 확정 결함).
fn dir_preview_lines(
    rows: &[SessionRow],
    node: &str,
    home: &str,
    now: i64,
    source: Option<&str>,
) -> Vec<Line<'static>> {
    const MAX_SESS: usize = 40;
    let mut total = 0usize;
    let mut child_counts: HashMap<String, usize> = HashMap::new();
    let mut child_order: Vec<String> = Vec::new();
    let mut sessions: Vec<(String, String, &str)> = Vec::new(); // (reltime, title, source)

    for r in rows {
        if source.is_some_and(|s| r.source != s) {
            continue;
        }
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
            sessions.push((model::reltime(r.mtime, now), r.title.clone(), r.source.as_str()));
        }
    }
    // child_order 는 첫 등장 순서 = mtime desc(rows 가 mtime desc) — Phase A 프리뷰와 동일. 정렬 안 함.

    let dim = theme::dim();
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
    lines.push(Line::from(Span::styled("최근 세션 — enter=이 폴더로 드릴:", dim)));
    for (rt, title, source) in &sessions {
        let (tag, color) = if *source == "codex" {
            ("x", theme::CODEX)
        } else {
            ("c", theme::CLAUDE)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {rt:>4} "), dim),
            Span::styled(format!("{tag} "), Style::default().fg(color)),
            Span::raw(title.clone()),
        ]));
    }
    if total > sessions.len() {
        lines.push(Line::from(Span::styled(
            format!("  … 외 {}개 (enter 로 드릴해서 검색)", total - sessions.len()),
            dim,
        )));
    }
    lines
}

/// 표시폭(셀) 기준 자르기 — CJK 2셀 문자를 반으로 자르지 않고, 초과 시 '…'(1셀) 부착.
fn truncate_width(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(1);
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > budget {
            break;
        }
        w += cw;
        out.push(ch);
    }
    out.push('…');
    out
}

/// 이벤트 루프 실행. 터미널 setup/teardown 포함. 반환 시 터미널은 이미 복원됨.
/// 클립보드 복사(pending_copy)는 여기서 수행 — handle_key 순수성 유지.
pub fn run(rows: Vec<SessionRow>, now: i64, home: String) -> Outcome {
    let mut term = ratatui::init();
    let mut app = App::new(rows, now, home);
    let outcome = loop {
        let _ = term.draw(|f| app.render(f));
        match event::read() {
            // Repeat 포함: kitty 프로토콜 터미널의 키 홀드 스크롤 유지(Release 만 배제).
            Ok(Event::Key(k)) if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                if let Some(o) = app.handle_key(k) {
                    break o;
                }
                if let Some(cmd) = app.pending_copy.take() {
                    app.status = Some(if resume::copy_to_clipboard(&cmd) {
                        format!("복사됨: {cmd}")
                    } else {
                        format!("⚠ pbcopy 실패 — 명령: {cmd}")
                    });
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
    fn buffer_text(term: &Terminal<TestBackend>) -> String {
        term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
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
        // ctrl-y 는 종료 대신 pending_copy 를 남긴다(이벤트 루프가 클립보드 실행)
        assert!(app.handle_key(ctrl('y')).is_none());
        assert_eq!(
            app.pending_copy.as_deref(),
            Some("cd '/home/u/work/b' && codex resume 'id'")
        );
        assert!(matches!(app.handle_key(plain(KeyCode::Esc)), Some(Outcome::Quit)));
    }

    #[test]
    fn enter_and_copy_rejected_without_cwd() {
        let mut bad = mkrow("claude", "no cwd", "/x", "x", 100);
        bad.cwd = None;
        let mut app = App::new(vec![bad], 1000, HOME.into());
        // enter → 종료 대신 status
        assert!(app.handle_key(plain(KeyCode::Enter)).is_none());
        assert!(app.status.as_deref().unwrap_or("").starts_with('⚠'));
        // ctrl-y → pending_copy 없음 + status
        app.handle_key(ctrl('y'));
        assert!(app.pending_copy.is_none());
        assert!(app.status.is_some());
        // 다음 키 입력에서 status 클리어
        app.handle_key(plain(KeyCode::Down));
        assert!(app.status.is_none());
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
    fn backspace_on_empty_query_goes_tree_up() {
        let mut app = app();
        app.handle_key(plain(KeyCode::Tab));
        app.prefix = "/home/u/work".into();
        app.recompute();
        app.handle_key(plain(KeyCode::Backspace)); // 빈 쿼리 → 상위
        assert!(app.prefix.is_empty());
        // Flat 빈 쿼리 backspace 는 no-op
        app.handle_key(ctrl('a'));
        app.handle_key(plain(KeyCode::Backspace));
        assert_eq!(app.view, ViewMode::Flat);
    }

    #[test]
    fn ctrl_g_toggles_scope_and_keeps_query() {
        let mut app = app();
        for c in "auth".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.items.len(), 2);
        app.sel = 0; // alpha @ /home/u/work/a
        app.handle_key(ctrl('g'));
        assert_eq!(app.scope.as_deref(), Some("/home/u/work/a"));
        assert_eq!(app.query, "auth"); // 쿼리 유지 — 스코프는 검색을 좁히는 조작
        assert_eq!(app.items.len(), 1); // 그 cwd 세션 + 쿼리 AND
        // 같은 세션에 다시 → 해제
        app.handle_key(ctrl('g'));
        assert_eq!(app.scope, None);
        assert_eq!(app.items.len(), 2);
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
        for k in ['g', 'h', 'a', 'o', 's', 'w', 'y'] {
            app.handle_key(ctrl(k));
        }
        assert_eq!(app.query, ""); // 어느 것도 쿼리에 안 들어감
    }

    #[test]
    fn ctrl_w_deletes_last_word() {
        let mut app = app();
        for c in "auth jwt".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.items.len(), 1);
        app.handle_key(ctrl('w'));
        assert_eq!(app.query, "auth ");
        app.handle_key(ctrl('w'));
        assert_eq!(app.query, "");
        assert_eq!(app.items.len(), 3); // 필터 해제
    }

    #[test]
    fn page_home_end_navigation() {
        let mut app = app();
        app.page = 2;
        app.handle_key(plain(KeyCode::PageDown));
        assert_eq!(app.sel, 2); // 0 + 2
        app.handle_key(plain(KeyCode::PageUp));
        assert_eq!(app.sel, 0);
        app.handle_key(plain(KeyCode::End));
        assert_eq!(app.sel, 2);
        app.handle_key(plain(KeyCode::Home));
        assert_eq!(app.sel, 0);
    }

    #[test]
    fn help_overlay_toggles_and_swallows_keys() {
        let mut app = app();
        app.handle_key(plain(KeyCode::F(1)));
        assert!(app.show_help);
        // 열린 상태에서 글자 키 → 닫힘 + 쿼리로 안 샘
        app.handle_key(key('a'));
        assert!(!app.show_help);
        assert_eq!(app.query, "");
        // alt-h 로도 열림, esc 는 닫기만(종료 아님)
        app.handle_key(alt('h'));
        assert!(app.show_help);
        assert!(app.handle_key(plain(KeyCode::Esc)).is_none());
        assert!(!app.show_help);
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
    fn prompt_label_static_in_flat_tree_shows_path() {
        let mut app = app();
        assert_eq!(app.prompt_label(), "search> ");
        app.handle_key(ctrl('s')); // 소스필터는 필로 — 프롬프트 불변
        assert_eq!(app.prompt_label(), "search> ");
        app.handle_key(plain(KeyCode::Tab));
        assert_eq!(app.prompt_label(), "~> ");
    }

    #[test]
    fn renders_flat_and_tree() {
        let backend = TestBackend::new(100, 20);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = app();
        term.draw(|f| app.render(f)).unwrap();
        let flat = buffer_text(&term);
        assert!(flat.contains("search>"));
        assert!(flat.contains("alpha"));
        assert!(flat.contains("3/3")); // 헤더 우측 카운트
        assert!(flat.contains("1 of 3")); // 리스트 보더 하단 커서 위치
        assert!(flat.contains("enter=resume")); // 푸터 힌트
        // 트리 진입 → 프롬프트 '~>' + 📁
        app.handle_key(plain(KeyCode::Tab));
        term.draw(|f| app.render(f)).unwrap();
        let tree = buffer_text(&term);
        assert!(tree.contains("~>"));
        assert!(tree.contains("📁"));
    }

    #[test]
    fn source_tabs_mark_active_and_scope_pill_renders() {
        let mut app = app();
        // 탭 활성 표시: 활성 탭만 소스색 fg, 비활성은 dim(fg 없음)
        let tab_fg = |app: &App, label: &str| {
            app.source_tabs()
                .spans
                .iter()
                .find(|s| s.content.as_ref() == label)
                .and_then(|s| s.style.fg)
        };
        assert_eq!(tab_fg(&app, "all"), Some(theme::NEUTRAL));
        assert_eq!(tab_fg(&app, "claude"), Some(theme::NEUTRAL)); // 비활성=NEUTRAL dim(보더색 상속 차단)
        app.handle_key(ctrl('s'));
        assert_eq!(tab_fg(&app, "claude"), Some(theme::CLAUDE));
        assert_eq!(tab_fg(&app, "all"), Some(theme::NEUTRAL));
        // 활성/비활성은 BOLD 로도 구분
        let bold = |app: &App, label: &str| {
            app.source_tabs()
                .spans
                .iter()
                .find(|s| s.content.as_ref() == label)
                .is_some_and(|s| s.style.add_modifier.contains(Modifier::BOLD))
        };
        assert!(bold(&app, "claude"));
        assert!(!bold(&app, "all"));
        // 스코프 필은 헤더 우측에 유지
        app.handle_key(ctrl('s'));
        app.handle_key(ctrl('s')); // 전체 복귀
        app.sel = 0;
        app.handle_key(ctrl('g')); // scope = /home/u/work/a
        let backend = TestBackend::new(100, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| app.render(f)).unwrap();
        let s = buffer_text(&term);
        assert!(s.contains(" work/a ")); // 스코프 필
        // 리스트 보더 탭 라벨도 렌더됨
        assert!(s.contains("all"));
        assert!(s.contains("codex"));
    }

    #[test]
    fn renders_empty_placeholder_with_filters() {
        let backend = TestBackend::new(100, 20);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = app();
        for c in "zzz없는단어".chars() {
            app.handle_key(key(c));
        }
        assert!(app.items.is_empty());
        // 안내 내용 자체는 라인 생성 함수로 검증(버퍼는 CJK 와이드문자 뒤에 공백 셀이 끼어 contains 불가)
        let flat: String = app
            .empty_state_lines()
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(flat.contains("매치 없음"));
        assert!(flat.contains("쿼리: zzz없는단어"));
        assert!(flat.contains("ctrl-a=전체 리셋"));
        // 렌더 경로: placeholder 가 실제로 그려짐 (ASCII 부분으로 확인)
        term.draw(|f| app.render(f)).unwrap();
        let s = buffer_text(&term);
        assert!(s.contains("backspace="));
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
        app.handle_key(plain(KeyCode::F(3)));
        assert_eq!(app.preview_scroll, 1); // F3 = 다음 매치 별칭(순환)
        assert_eq!(app.query, "auth"); // Alt+n/p·F3 은 쿼리에 안 샘
    }

    #[test]
    fn query_change_invalidates_query_caches() {
        let mut app = app();
        let path = "/no/such/path.jsonl".to_string();
        app.preview_cache.insert(path.clone(), vec![Line::from("auth 라인")]);
        for c in "auth".chars() {
            app.handle_key(key(c));
        }
        let _ = app.preview_content();
        assert!(app.hl_cache.contains_key(&format!("{path}\u{0}auth")));
        app.handle_key(plain(KeyCode::Backspace)); // "aut" — 세대 교체
        assert!(app.hl_cache.is_empty()); // 옛 쿼리 키 잔류 없음(무한 성장 차단)
        assert!(app.match_cache.is_empty());
    }

    #[test]
    fn match_counter_follows_scroll() {
        let mut app = app();
        let path = "/no/such/path.jsonl".to_string();
        let mut lines: Vec<Line> = (0..20).map(|i| Line::from(format!("filler {i}"))).collect();
        for &i in &[3usize, 8, 14] {
            lines[i] = Line::from("auth 여기");
        }
        app.preview_cache.insert(path, lines);
        for c in "auth".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.preview_match_counter(), Some((1, 3)));
        app.handle_key(alt('n'));
        assert_eq!(app.preview_match_counter(), Some((2, 3)));
        app.handle_key(alt('n'));
        assert_eq!(app.preview_match_counter(), Some((3, 3)));
    }

    #[test]
    fn ctrl_d_clamps_at_content_end_after_render() {
        let backend = TestBackend::new(100, 20);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = app();
        let path = "/no/such/path.jsonl".to_string();
        let lines: Vec<Line> = (0..30).map(|i| Line::from(format!("l{i}"))).collect();
        app.preview_cache.insert(path, lines);
        term.draw(|f| app.render(f)).unwrap(); // 랩 캐시 + pv_height 충전
        for _ in 0..50 {
            app.handle_key(ctrl('d')); // 500줄 상당 — 콘텐츠(30줄)에서 클램프
        }
        assert!(app.preview_scroll <= 30, "scroll={}", app.preview_scroll);
        // 되돌아오기는 즉시(과주행 잔량 없음)
        app.handle_key(ctrl('u'));
        assert!(app.preview_scroll <= 20);
    }

    #[test]
    fn match_cycling_survives_render_clamp() {
        // 리뷰 확정 결함 재현: 콘텐츠 끝 뷰포트 안 매치에서 alt-n 이 고착되지 않고 순환해야 한다.
        let backend = TestBackend::new(100, 20);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = app();
        let path = "/no/such/path.jsonl".to_string();
        // 40줄, 매치 = 5·35. vh≈16 → max_scroll≈24 < 33(매치35 앵커) — 클램프 구간에 매치.
        let mut lines: Vec<Line> = (0..40).map(|i| Line::from(format!("filler {i}"))).collect();
        lines[5] = Line::from("auth 앞");
        lines[35] = Line::from("auth 끝");
        app.preview_cache.insert(path, lines);
        for c in "auth".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.preview_scroll, 3); // 첫 매치 5 - LEAD_IN
        term.draw(|f| app.render(f)).unwrap();
        app.handle_key(alt('n')); // → 매치 35 (논리 scroll 33)
        assert_eq!(app.preview_scroll, 33);
        term.draw(|f| app.render(f)).unwrap(); // 표시 클램프가 논리값을 덮어쓰면 안 됨
        assert_eq!(app.preview_scroll, 33);
        assert_eq!(app.preview_match_counter(), Some((2, 2)));
        app.handle_key(alt('n')); // 끝에서 → 첫 매치로 순환 (고착 금지)
        assert_eq!(app.preview_scroll, 3);
        assert_eq!(app.preview_match_counter(), Some((1, 2)));
    }

    #[test]
    fn dir_preview_respects_source_filter() {
        let mut app = app();
        app.handle_key(ctrl('s')); // Claude만
        let lines = dir_preview_lines(&app.rows, "/home/u/work", HOME, 1000, Some("claude"));
        let flat: String = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(flat.contains("2 세션")); // codex(beta) 제외
        assert!(!flat.contains("beta deploy"));
        // 캐시 키가 필터 차원을 포함 → 필터별 별개 엔트리(스테일 없음)
        assert_ne!(app.dir_key("/home/u/work"), {
            app.handle_key(ctrl('s')); // Codex만
            app.dir_key("/home/u/work")
        });
    }

    #[test]
    fn truncate_width_cjk_and_ascii() {
        assert_eq!(truncate_width("abcdef", 10), "abcdef"); // 여유 → 그대로
        assert_eq!(truncate_width("abcdef", 4), "abc…");
        assert_eq!(truncate_width("가나다라", 20), "가나다라");
        // 폭 5: '…'(1) 예산 4 → 2셀 문자 2개
        assert_eq!(truncate_width("가나다라", 5), "가나…");
        // 2셀 문자를 반으로 안 자름: 예산 3 → 1개(2셀)만
        assert_eq!(truncate_width("가나다라", 4), "가…");
    }

    #[test]
    fn row_spans_right_aligns_proj_and_marks_body_only() {
        let app = app();
        let lw = 60usize;
        // 제목 매치 없음 + 본문 매치 상황 시뮬레이션: terms 가 제목에 없음
        let terms = vec!["zzz".to_string()];
        let spans = app.row_spans(&ListItem::Session { row_idx: 0 }, false, lw, &terms);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert!(joined.contains("·본문")); // 본문-only 마커
        assert!(joined.ends_with("work/a")); // proj 우측 정렬(끝)
        let w: usize = spans.iter().map(|s| s.content.as_ref().width()).sum();
        assert_eq!(w, lw); // 행 폭 = 예산 (패딩 포함)
        // 제목 매치 → WARN 강조 스팬 존재 + 마커 없음
        let terms = vec!["auth".to_string()];
        let spans = app.row_spans(&ListItem::Session { row_idx: 0 }, false, lw, &terms);
        assert!(spans.iter().any(|s| s.content.as_ref() == "auth" && s.style.fg == Some(theme::WARN)));
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert!(!joined.contains("·본문"));
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
