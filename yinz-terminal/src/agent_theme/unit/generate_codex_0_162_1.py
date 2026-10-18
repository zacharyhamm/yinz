# Regenerate with: python3 generate_codex_0_162_1.py /path/to/codex-rust-v0.162.1
# Runs the pinned provider's source with a minimal ratatui shim; never imports Yinz's math.
from pathlib import Path
import subprocess, json, sys, tempfile
root=Path(sys.argv[1]) / 'codex-rs/tui/src'
temporary = tempfile.TemporaryDirectory(prefix='codex-color-reference-')
out=Path(temporary.name)
(out/'color.rs').write_text((root/'color.rs').read_text())
(out/'provider_style').mkdir(exist_ok=True)
(out/'provider_style/contrast.rs').write_text((root/'style/contrast.rs').read_text().split('#[cfg(test)]')[0])
(out/'provider_style.rs').write_text((root/'style.rs').read_text().split('#[cfg(test)]')[0].replace('mod contrast;', '#[path = "provider_style/contrast.rs"]\nmod contrast;'))
palette=(root/'terminal_palette.rs').read_text()
func=palette[palette.index('fn best_color_for_color_level'):palette.index('#[derive(Clone, Copy)]')]
colors=palette[palette.index('pub const XTERM_COLORS'):palette.index('#[cfg(test)]\nmod tests')]
(out/'terminal_palette.rs').write_text('''use crate::style::Color;
use crate::color::perceptual_distance;
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StdoutColorLevel { TrueColor, Ansi256, Ansi16, Unknown }
thread_local! { pub static PALETTE: std::cell::Cell<((u8,u8,u8),(u8,u8,u8),StdoutColorLevel)> = const { std::cell::Cell::new(((0,0,0),(0,0,0),StdoutColorLevel::TrueColor)) }; }
pub fn stdout_color_level() -> StdoutColorLevel { PALETTE.with(|p|p.get().2) }
pub fn effective_stdout_color_level() -> StdoutColorLevel { stdout_color_level() }
pub fn default_fg() -> Option<(u8,u8,u8)> { Some(PALETTE.with(|p|p.get().0)) }
pub fn default_bg() -> Option<(u8,u8,u8)> { Some(PALETTE.with(|p|p.get().1)) }
pub fn rgb_color((r,g,b):(u8,u8,u8)) -> Color { Color::Rgb(r,g,b) }
pub fn indexed_color(i:u8) -> Color { Color::Indexed(i) }
pub fn best_color(c:(u8,u8,u8)) -> Color { best_color_for_color_level(c, stdout_color_level()) }
pub fn best_color_for_level(c:(u8,u8,u8),l:StdoutColorLevel) -> Color { best_color_for_color_level(c,l) }
fn xterm_fixed_colors() -> impl Iterator<Item = (usize, (u8, u8, u8))> { XTERM_COLORS.into_iter().enumerate().skip(16) }
'''+func+colors)
footer=(root/'bottom_pane/status_line_style.rs').read_text()
soften=footer[footer.index('fn soften_status_line_color'):footer.index('#[cfg(test)]')]
(out/'footer.rs').write_text('use crate::style::Color;\nconst STATUS_LINE_COLOR_SATURATION_PERCENT: u16 = 85;\nconst STATUS_LINE_COLOR_BRIGHTNESS_PERCENT: u16 = 100;\n'+soften.replace('fn soften_status_line_color', 'pub fn soften_status_line_color'))
(out/'main.rs').write_text('''extern crate self as ratatui;
mod color; mod terminal_palette; mod provider_style; mod footer;
pub mod style {
#[derive(Clone, Copy, Debug, Default)] pub enum Color { #[default] Reset, Rgb(u8,u8,u8), Indexed(u8), Black, Red, Green, Yellow, Blue, Magenta, Cyan, Gray, DarkGray, LightRed, LightGreen, LightYellow, LightBlue, LightMagenta, LightCyan, White }
#[derive(Clone, Copy, Default)] pub struct Style { pub fg: Option<Color>, pub bg: Option<Color> }
pub struct Modifier; impl Modifier { pub const DIM: Self = Self; }
impl Style {
pub fn fg(mut self,c:Color)->Self{self.fg=Some(c);self} pub fn bg(mut self,c:Color)->Self{self.bg=Some(c);self}
pub fn bold(self)->Self{self} pub fn not_bold(self)->Self{self} pub fn dim(self)->Self{self} pub fn not_dim(self)->Self{self}
pub fn reversed(self)->Self{self} pub fn not_reversed(self)->Self{self} pub fn remove_modifier(self,_:Modifier)->Self{self}
}}
mod render { pub mod highlight { pub fn foreground_style_for_scopes(_: &[&str])->Option<crate::style::Style>{None} } }
fn emit(name:&str, c:Option<style::Color>){ println!("{name} {:?}",c.unwrap()); }
fn main(){
use terminal_palette::StdoutColorLevel::*;
for (name,fg,bg) in [
("dark",(220,220,220),(18,20,30)),("light",(20,20,20),(255,255,255)),
("nightfox",(205,206,207),(25,35,48)),
("tinted",(215,220,230),(42,58,72)),("tinted_light",(50,40,30),(245,240,220)),
("midtone",(220,220,220),(130,130,130)),("blue",(0,0,0),(164,205,251)),
("black",(255,255,255),(0,0,0))] {
for level in [TrueColor,Ansi256] {
terminal_palette::PALETTE.with(|p|p.set((fg,bg,level)));
println!("PALETTE {name} {level:?} {fg:?} {bg:?}");
emit("composer-plan",provider_style::user_message_style().bg);
emit("history-prompt",provider_style::history_prompt_style().bg);
emit("secondary",provider_style::secondary_text_style().fg);
emit("table-separator",provider_style::table_separator_style().fg);
emit("warning",provider_style::warning_notice_style().fg);
emit("accent",Some(provider_style::accent_color()));
for (name,rgb) in [
("mocha.model",(249,226,175)),("mocha.path",(166,227,161)),
("latte.model",(223,142,29)),("latte.path",(64,160,43)),
("custom.model",(240,210,160)),("custom.path",(190,230,180))] {
let softened=footer::soften_status_line_color(style::Color::Rgb(rgb.0,rgb.1,rgb.2));
emit(&format!("footer-source.{name}"),Some(softened));
emit(&format!("footer.{name}"),Some(provider_style::readable_color_on(softened,None)));
}
let s=provider_style::selection_style(); emit("selection.foreground",s.fg);emit("selection.background",s.bg);
emit("selection.secondary",Some(provider_style::readable_color_on(provider_style::secondary_text_style().fg.unwrap(),s.bg)));
for (name,surface) in [("composer",provider_style::user_message_style().bg),("history",provider_style::history_prompt_style().bg)] {
emit(&format!("{name}-accent"),Some(provider_style::accent_color_on(surface)));
}
}}
}
''')
subprocess.run(['rustc','--edition=2024','-Awarnings',str(out/'main.rs'),'-o',str(out/'reference')], check=True)
raw=subprocess.check_output([str(out/'reference')],text=True)
import re
fixtures=[]
for line in raw.splitlines():
    if line.startswith('PALETTE '):
        _,name,level,*_=line.split()
        nums=list(map(int,re.findall(r'\d+', line[line.index('('):])))
        entry={'name':name,'indexed':level=='Ansi256','foreground':nums[:3],'background':nums[3:],'colors':{}}
        fixtures.append(entry)
    else:
        key,value=line.split(' ',1)
        nums=list(map(int,re.findall(r'\d+',value)))
        entry['colors'][key]={'Rgb':nums} if value.startswith('Rgb') else {'Index':nums[0]}
Path(__file__).with_name('codex-0.162.1-colors.json').write_text(json.dumps(fixtures,indent=2)+'\n')
