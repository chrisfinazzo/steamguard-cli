use anyhow::Context;
use crossterm::{
	cursor,
	event::{Event, KeyCode, KeyEvent, KeyModifiers},
	execute,
	style::{Color, Print, PrintStyledContent, SetForegroundColor, Stylize},
	terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
	QueueableCommand,
};
use log::debug;
use secrecy::SecretString;
use std::collections::HashSet;
use std::io::{stderr, stdout, Write};
use steamguard::Confirmation;

/// Prompt the user for text input.
pub(crate) fn prompt() -> String {
	stdout().flush().expect("failed to flush stdout");
	stderr().flush().expect("failed to flush stderr");

	let mut line = String::new();
	while let Event::Key(KeyEvent { code, .. }) = crossterm::event::read().unwrap() {
		match code {
			KeyCode::Enter => {
				eprintln!();
				break;
			}
			KeyCode::Char(c) => {
				line.push(c);
				eprint!("{}", c);
				let _ = stderr().flush();
			}
			KeyCode::Backspace => {
				if !line.is_empty() {
					line.pop();

					eprint!("\x08 \x08");
					let _ = stderr().flush();
				}
			}
			_ => {}
		}
	}

	line
}

pub(crate) fn prompt_allow_empty(prompt_text: impl AsRef<str>) -> String {
	eprint!("{}", prompt_text.as_ref());
	prompt()
}

pub(crate) fn prompt_non_empty(prompt_text: impl AsRef<str>) -> String {
	loop {
		eprint!("{}", prompt_text.as_ref());
		let input = prompt();
		if !input.is_empty() {
			return input;
		}
	}
}

/// Prompt the user for a single character response. Useful for asking yes or no questions.
///
/// `chars` should be all lowercase characters, with at most 1 uppercase character. The uppercase character is the default answer if no answer is provided.
/// The selected character returned will always be lowercase.
pub(crate) fn prompt_char(text: &str, chars: &str) -> char {
	loop {
		let _ = stderr().queue(Print(format!("{} [{}] ", text, chars)));
		let _ = stderr().flush();
		let input = prompt();
		if let Ok(c) = prompt_char_impl(input, chars) {
			return c;
		}
	}
}

fn prompt_char_impl(input: impl Into<String>, chars: &str) -> anyhow::Result<char> {
	let uppers = chars.replace(char::is_lowercase, "");
	if uppers.len() > 1 {
		panic!("Invalid chars for prompt_char. Maximum 1 uppercase letter is allowed.");
	}
	let default_answer: Option<char> = if uppers.len() == 1 {
		Some(uppers.chars().collect::<Vec<char>>()[0].to_ascii_lowercase())
	} else {
		None
	};

	let answer: String = input.into().to_ascii_lowercase();

	if answer.is_empty() {
		if let Some(a) = default_answer {
			return Ok(a);
		} else {
			bail!("no valid answer")
		}
	} else if answer.len() > 1 {
		bail!("answer too long")
	}

	let answer_char = answer.chars().collect::<Vec<char>>()[0];
	if chars.to_ascii_lowercase().contains(answer_char) {
		return Ok(answer_char);
	}

	bail!("no valid answer")
}

/// Returns a tuple of (accepted, denied). Ignored confirmations are not included.
pub(crate) fn prompt_confirmation_menu(
	confirmations: Vec<Confirmation>,
) -> anyhow::Result<(Vec<Confirmation>, Vec<Confirmation>)> {
	if confirmations.is_empty() {
		return Ok((vec![], vec![]));
	}

	let mut to_accept_idx: HashSet<usize> = HashSet::new();
	let mut to_deny_idx: HashSet<usize> = HashSet::new();

	execute!(stdout(), EnterAlternateScreen)?;
	crossterm::terminal::enable_raw_mode()?;

	let mut selected_idx = 0;

	loop {
		execute!(
			stdout(),
			Clear(ClearType::All),
			cursor::MoveTo(1, 1),
			PrintStyledContent(
				"arrow keys to select, [a]ccept, [d]eny, [i]gnore, [enter] confirm choices\n\n"
					.white()
			),
		)?;

		for (i, conf) in confirmations.iter().enumerate() {
			stdout().queue(Print("\r"))?;
			if selected_idx == i {
				stdout().queue(SetForegroundColor(Color::Yellow))?;
				stdout().queue(Print(" >"))?;
			} else {
				stdout().queue(SetForegroundColor(Color::White))?;
				stdout().queue(Print("  "))?;
			}

			if to_accept_idx.contains(&i) {
				stdout().queue(SetForegroundColor(Color::Green))?;
				stdout().queue(Print("[a]"))?;
			} else if to_deny_idx.contains(&i) {
				stdout().queue(SetForegroundColor(Color::Red))?;
				stdout().queue(Print("[d]"))?;
			} else {
				stdout().queue(Print("[ ]"))?;
			}

			if selected_idx == i {
				stdout().queue(SetForegroundColor(Color::Yellow))?;
			}

			stdout().queue(Print(format!(" {}\n", conf.description())))?;
		}

		stdout().flush()?;

		match crossterm::event::read()? {
			Event::Resize(_, _) => continue,
			Event::Key(KeyEvent {
				code: KeyCode::Char('a'),
				..
			}) => {
				to_accept_idx.insert(selected_idx);
				to_deny_idx.remove(&selected_idx);
			}
			Event::Key(KeyEvent {
				code: KeyCode::Char('d'),
				..
			}) => {
				to_accept_idx.remove(&selected_idx);
				to_deny_idx.insert(selected_idx);
			}
			Event::Key(KeyEvent {
				code: KeyCode::Char('i'),
				..
			}) => {
				to_accept_idx.remove(&selected_idx);
				to_deny_idx.remove(&selected_idx);
			}
			Event::Key(KeyEvent {
				code: KeyCode::Char('A'),
				..
			}) => {
				(0..confirmations.len()).for_each(|i| {
					to_accept_idx.insert(i);
					to_deny_idx.remove(&i);
				});
			}
			Event::Key(KeyEvent {
				code: KeyCode::Char('D'),
				..
			}) => {
				(0..confirmations.len()).for_each(|i| {
					to_accept_idx.remove(&i);
					to_deny_idx.insert(i);
				});
			}
			Event::Key(KeyEvent {
				code: KeyCode::Char('I'),
				..
			}) => {
				(0..confirmations.len()).for_each(|i| {
					to_accept_idx.remove(&i);
					to_deny_idx.remove(&i);
				});
			}
			Event::Key(KeyEvent {
				code: KeyCode::Up, ..
			}) if selected_idx > 0 => {
				selected_idx -= 1;
			}
			Event::Key(KeyEvent {
				code: KeyCode::Down,
				..
			}) if selected_idx < confirmations.len() - 1 => {
				selected_idx += 1;
			}
			Event::Key(KeyEvent {
				code: KeyCode::Enter,
				..
			}) => {
				break;
			}
			Event::Key(KeyEvent {
				code: KeyCode::Esc, ..
			})
			| Event::Key(KeyEvent {
				code: KeyCode::Char('c'),
				modifiers: KeyModifiers::CONTROL,
			}) => {
				return Ok((vec![], vec![]));
			}
			_ => {}
		}
	}

	execute!(stdout(), LeaveAlternateScreen)?;
	crossterm::terminal::disable_raw_mode()?;

	Ok((
		to_accept_idx
			.iter()
			.map(|i| confirmations[*i].clone())
			.collect(),
		to_deny_idx
			.iter()
			.map(|i| confirmations[*i].clone())
			.collect(),
	))
}

pub(crate) fn pause() {
	let _ = write!(stderr(), "Press enter to continue...");
	let _ = stderr().flush();
	loop {
		match crossterm::event::read().expect("could not read terminal events") {
			Event::Key(KeyEvent {
				code: KeyCode::Enter,
				..
			}) => break,
			_ => continue,
		}
	}
}

pub(crate) fn prompt_passkey() -> anyhow::Result<SecretString> {
	debug!("prompting for passkey");
	loop {
		let raw = rpassword::prompt_password("Enter encryption passkey: ")
			.context("prompting for passkey")?;
		if !raw.is_empty() {
			return Ok(SecretString::new(raw));
		}
	}
}

pub(crate) fn prompt_password() -> anyhow::Result<SecretString> {
	debug!("prompting for password");
	loop {
		let raw = rpassword::prompt_password("Password: ").context("prompting for password")?;
		if !raw.is_empty() {
			return Ok(SecretString::new(raw));
		}
	}
}

#[cfg(test)]
mod prompt_char_tests {
	use super::*;

	#[test]
	fn test_gives_answer() {
		let answer = prompt_char_impl("y", "yn").unwrap();
		assert_eq!(answer, 'y');
	}

	#[test]
	fn test_gives_default() {
		let answer = prompt_char_impl("", "Yn").unwrap();
		assert_eq!(answer, 'y');
	}

	#[test]
	fn test_should_not_give_default() {
		let answer = prompt_char_impl("n", "Yn").unwrap();
		assert_eq!(answer, 'n');
	}

	#[test]
	fn test_should_not_give_invalid() {
		let answer = prompt_char_impl("g", "yn");
		assert!(answer.is_err());
		let answer = prompt_char_impl("n", "yn").unwrap();
		assert_eq!(answer, 'n');
	}

	#[test]
	fn test_should_not_give_multichar() {
		let answer = prompt_char_impl("yy", "yn");
		assert!(answer.is_err());
	}
}
