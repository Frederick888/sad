use super::errors::*;
use super::subprocess::SubprocessCommand;
use aho_corasick::{AhoCorasick, AhoCorasickBuilder};
use regex::{Regex, RegexBuilder};
use std::{collections::HashMap, env, fs, path::PathBuf};
use structopt::StructOpt;

#[derive(StructOpt)]
#[structopt(name = "sad", author, about)]
pub struct Arguments {
  /// Search pattern
  #[structopt()]
  pub pattern: String,

  /// Replacement pattern, empty = delete
  #[structopt()]
  pub replace: Option<String>,

  /// Use \0 as stdin delimiter
  #[structopt(short = "0", long = "read0")]
  pub nul_delim: bool,

  /// No preview, write changes to file
  #[structopt(short = "k", long)]
  pub commit: bool,

  /// String literal mode
  #[structopt(short, long)]
  pub exact: bool,

  /// Standard regex flags: ie. -f imx, full list: https://github.com/ms-jpq/sad
  #[structopt(short, long)]
  pub flags: Option<String>,

  /// Colourizing program, disable = never, default = $GIT_PAGER
  #[structopt(short, long)]
  pub pager: Option<String>,

  /// Additional Fzf options, disable = never
  #[structopt(long)]
  pub fzf: Option<String>,

  /// Same as in GNU diff --unified={size}, affects hunk size
  #[structopt(short, long)]
  pub unified: Option<usize>,

  /// *Internal use only*
  #[structopt(long)]
  pub internal_preview: Option<String>,

  /// *Internal use only*
  #[structopt(long)]
  pub internal_patch: Option<Vec<String>>,

  /// *Internal use only*
  #[structopt(short = "c")]
  pub shell: Option<String>,
}

impl Arguments {
  pub fn new() -> Arguments {
    let args = env::args().collect::<Vec<_>>();
    match (args.get(0), args.get(1), args.get(2)) {
      (Some(name), Some(lhs), Some(rhs)) if lhs == "-c" => {
        let params = rhs.split("\x04").enumerate().collect::<Vec<_>>();
        let last = params.len() - 1;
        let mut args = vec![name.to_owned()];
        for (i, param) in params {
          if i == last {
            let splits = shlex::split(param).unwrap_or(Vec::new());
            args.extend(splits)
          } else {
            args.push(param.to_owned())
          }
        }
        Arguments::from_iter(args)
      }
      _ => Arguments::from_args(),
    }
  }
}

#[derive(Clone, Debug)]
pub enum Engine {
  AhoCorasick(AhoCorasick, String),
  Regex(Regex, String),
}

#[derive(Clone, Debug)]
pub enum Action {
  Preview,
  Commit,
  Fzf,
}

#[derive(Clone, Debug)]
pub enum Printer {
  Stdout,
  Pager(SubprocessCommand),
}

#[derive(Clone, Debug)]
pub struct Options {
  pub name: String,
  pub action: Action,
  pub engine: Engine,
  pub fzf: Option<Vec<String>>,
  pub printer: Printer,
  pub unified: usize,
}

impl Options {
  pub fn new(args: Arguments) -> SadResult<Options> {
    let name = env::args()
      .next()
      .and_then(|s| {
        let path = PathBuf::from(s);
        fs::canonicalize(path).ok()
      })
      .or(which::which("sad").ok())
      .unwrap_or(PathBuf::from("sad"))
      .to_string_lossy()
      .to_string();

    let mut flagset = p_auto_flags(&args.pattern);
    flagset.extend(
      args
        .flags
        .unwrap_or_default()
        .split_terminator("")
        .skip(1)
        .map(String::from),
    );

    let engine = {
      let replace = args.replace.unwrap_or_default();
      if args.exact {
        Engine::AhoCorasick(p_aho_corasick(&args.pattern, &flagset)?, replace)
      } else {
        Engine::Regex(p_regex(&args.pattern, &flagset)?, replace)
      }
    };

    let fzf = p_fzf(args.fzf);

    let action = if args.commit || args.internal_patch != None {
      Action::Commit
    } else if args.internal_preview != None || fzf == None {
      Action::Preview
    } else {
      Action::Fzf
    };

    let printer = match p_pager(args.pager) {
      Some(cmd) => Printer::Pager(cmd),
      None => Printer::Stdout,
    };

    Ok(Options {
      name,
      action,
      engine,
      fzf,
      printer,
      unified: args.unified.unwrap_or(3),
    })
  }
}

fn p_auto_flags(pattern: &str) -> Vec<String> {
  for c in pattern.chars() {
    if c.is_uppercase() {
      return vec!["I".into()];
    }
  }
  vec!["i".into()]
}

fn p_aho_corasick(pattern: &str, flags: &[String]) -> SadResult<AhoCorasick> {
  let mut ac = AhoCorasickBuilder::new();
  for flag in flags {
    match flag.as_str() {
      "I" => ac.ascii_case_insensitive(false),
      "i" => ac.ascii_case_insensitive(true),
      _ => return Err(Failure::Simple("Invalid flags".into())),
    };
  }
  Ok(ac.build(&[pattern]))
}

fn p_regex(pattern: &str, flags: &[String]) -> SadResult<Regex> {
  let mut re = RegexBuilder::new(pattern);
  for flag in flags {
    match flag.as_str() {
      "I" => re.case_insensitive(false),
      "i" => re.case_insensitive(true),
      "m" => re.multi_line(true),
      "s" => re.dot_matches_new_line(true),
      "U" => re.swap_greed(true),
      "x" => re.ignore_whitespace(true),
      _ => return Err(Failure::Simple("Invalid flags".into())),
    };
  }
  re.build().into_sadness()
}

fn p_tty() -> bool {
  atty::is(atty::Stream::Stdout)
}

fn p_fzf(fzf: Option<String>) -> Option<Vec<String>> {
  match (which::which("fzf"), p_tty()) {
    (Ok(_), true) => match fzf {
      Some(v) if v == "never" => None,
      Some(val) => Some(val.split_whitespace().map(String::from).collect()),
      None => Some(Vec::new()),
    },
    _ => None,
  }
}

fn p_pager(pager: Option<String>) -> Option<SubprocessCommand> {
  pager.or(env::var("GIT_PAGER").ok()).and_then(|val| {
    if val == "never" {
      None
    } else {
      let less_less = val.split('|').next().unwrap_or(&val).trim();
      let mut commands = less_less.split_whitespace().map(String::from);
      commands.next().map(|program| SubprocessCommand {
        program,
        arguments: commands.collect(),
        env: HashMap::new(),
      })
    }
  })
}
