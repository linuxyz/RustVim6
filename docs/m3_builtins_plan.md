# M3 builtin-function implementation plan

Baseline denominator: 543 functions (`evalfunc.c` global_functions[]). Target: >= 70% (381 implemented).
Already implemented (77 pure in `builtins.rs` + 24 host in `rvim-ex::script`) are excluded below.

## Rules for every group module

- Create/edit exactly one file: `crates/rvim-viml/src/builtins_<group>.rs` (already registered in `lib.rs` and aggregated by `builtins.rs`).
- Export: `pub const FUNCS: &[&str]` (names you implement) and
  `pub fn call(name: &str, args: &[Value], env: &mut Env, host: &mut dyn Host) -> Option<ValueResult>`.
- Return `None` for names not in your FUNCS list; `Some(Err(..))` only for real Vim errors.
- Validate arity with `crate::builtins::check_arity(name, args)?`.
- Vim semantics: i64 numbers; UTF-8 strings with Vim's byte-vs-char index rules (`strpart`/`strlen` are byte-based,
  `strchars`/`charidx` char-based); negative indexes count from the end; helpers `Value::to_string_value()`,
  `to_number()`, `as_number()`, `truthy()`, `type_name()`; containers are `Rc<RefCell<..>>`.
- SKIP functions that need unbuilt subsystems: `ch_*`, `job_*`, `term_*`, `popup_*`, `sign_*`, `prop_*`,
  `win_*`, `tab*`, `gettabwinvar`, `complete*`, `balloon_*`, `browse*`, `test_*`, `synID*`, `hlID`, `hlexists`,
  `remote*`, `server*`, `sockconnect`, `listener_*`, `dictwatcheradd`. Leave them out of FUNCS.
- Add at least one unit test per implemented function under `#[cfg(test)] mod tests`.
- Verify: `cargo build -p rvim-viml` and `cargo test -p rvim-viml` (other agents edit other files in parallel;
  ignore failures that clearly come from files you do not own, but your file must compile).
- Also run `cargo run -p rvim-tools -- coverage` to see the aggregate count go up.

## Group a-e (91 candidate names)

```text
append appendbufline argc argidx arglistid argv assert_beeps assert_equal assert_equalfile
assert_exception assert_fails assert_false assert_inrange assert_match assert_nobeep assert_notequal
assert_notmatch assert_report assert_true autocmd_add autocmd_delete autocmd_get balloon_gettext
balloon_show balloon_split blob2list browse browsedir bufadd bufexists buffer_exists buffer_name
buffer_number buflisted bufload bufloaded bufwinid bufwinnr byte2line byteidx byteidxcomp call
ch_canread ch_close ch_close_in ch_evalexpr ch_evalraw ch_getbufnr ch_getjob ch_info ch_log
ch_logfile ch_open ch_read ch_readblob ch_readraw ch_sendexpr ch_sendraw ch_setoptions ch_status
changenr charclass charcol charidx chdir cindent clearmatches complete complete_add complete_check
complete_info confirm cosh cscope_connection cursor debugbreak delete deletebufline diff_filler
diff_hlID digraph_get digraph_getlist digraph_set digraph_setlist echoraw environ eventhandler
exepath exists_compiled expandcmd extendnew
```

## Group f-l (110 candidate names)

```text
feedkeys file_readable filewritable finddir findfile flatten flattennew foldclosed foldclosedend
foldlevel foldtext foldtextresult foreground fullcommand funcref garbagecollect getbufinfo
getbufline getbufvar getchangelist getchar getcharmod getcharpos getcharsearch getcharstr
getcmdcompltype getcmdline getcmdpos getcmdscreenpos getcmdtype getcmdwintype getcompletion
getcurpos getcursorcharpos getenv getfontname getfperm getfsize getftime getftype getimstatus
getjumplist getloclist getmarklist getmatches getmousepos getpid getpos getqflist getreg getreginfo
getregtype gettabinfo gettabvar gettabwinvar gettagstack gettext getwininfo getwinpos getwinposx
getwinposy getwinvar glob glob2regpat globpath haslocaldir hasmapto highlight_exists highlightID
histadd histdel histget histnr hlexists hlget hlID hlset hostname iconv indent input inputdialog
inputlist inputrestore inputsave inputsecret interrupt isabsolutepath islocked job_getchannel
job_info job_setoptions job_start job_status job_stop js_decode js_encode json_decode json_encode
last_buffer_nr libcall libcallnr line2byte lispindent list2blob list2str listener_add listener_flush
listener_remove luaeval
```

## Group m-r (81 candidate names)

```text
maparg mapcheck maplist mapnew mapset matchadd matchaddpos matcharg matchdelete matchfuzzy
matchfuzzypos matchlist matchstrpos menu_info mkdir mzeval nextnonblank pathshorten perleval
popup_atcursor popup_beval popup_clear popup_close popup_create popup_dialog popup_filter_menu
popup_filter_yesno popup_findinfo popup_findpreview popup_getoptions popup_getpos popup_hide
popup_list popup_locate popup_menu popup_move popup_notification popup_setoptions popup_settext
popup_show prevnonblank prompt_getprompt prompt_setcallback prompt_setinterrupt prompt_setprompt
prop_add prop_add_list prop_clear prop_find prop_list prop_remove prop_type_add prop_type_change
prop_type_delete prop_type_get prop_type_list pum_getpos pumvisible py3eval pyeval pyxeval rand
readblob readdir readdirex readfile reduce reg_executing reg_recording reltime reltimefloat
reltimestr remote_expr remote_foreground remote_peek remote_read remote_send remote_startserver
rename resolve rubyeval
```

## Group s1 (82 candidate names)

```text
screenattr screenchar screenchars screencol screenpos screenrow screenstring search searchcount
searchdecl searchpair searchpairpos searchpos server2client serverlist setbufline setbufvar
setcellwidths setcharpos setcharsearch setcmdpos setcursorcharpos setenv setfperm setloclist
setmatches setpos setqflist setreg settabvar settabwinvar settagstack setwinvar sha256 shellescape
shiftwidth sign_define sign_getdefined sign_getplaced sign_jump sign_place sign_placelist
sign_undefine sign_unplace sign_unplacelist simplify sinh slice sound_clear sound_playevent
sound_playfile sound_stop soundfold spellbadword spellsuggest srand state str2list strcharlen
strdisplaywidth strgetchar strptime strtrans submatch swapinfo swapname synconcealed synID synIDattr
synIDtrans synstack system systemlist tabpagebuflist tabpagewinnr tagfiles taglist tanh tempname
term_dumpdiff term_dumpload term_dumpwrite
```

## Group s2 (82 candidate names)

```text
term_getaltscreen term_getansicolors term_getattr term_getcursor term_getjob term_getline
term_getscrolled term_getsize term_getstatus term_gettitle term_gettty term_list term_scrape
term_sendkeys term_setansicolors term_setapi term_setkill term_setrestore term_setsize term_start
term_wait terminalprops test_alloc_fail test_autochdir test_feedinput test_garbagecollect_now
test_garbagecollect_soon test_getvalue test_gui_event test_ignore_error test_null_blob
test_null_channel test_null_dict test_null_function test_null_job test_null_list test_null_partial
test_null_string test_option_not_set test_override test_refcount test_setmouse test_settime
test_srand_seed test_unknown test_void timer_info timer_pause timer_start timer_stop timer_stopall
tr typename undofile undotree virtcol virtcol2col visualmode wildmenumode win_execute win_findbuf
win_getid win_gettype win_gotoid win_id2tabwin win_id2win win_move_separator win_move_statusline
win_screenpos win_splitmove winbufnr wincol windowsversion winheight winlayout winline winrestcmd
winrestview winsaveview winwidth wordcount writefile
```


