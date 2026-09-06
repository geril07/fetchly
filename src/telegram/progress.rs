use std::time::{Duration, Instant};

use teloxide::prelude::*;
use teloxide::types::{ChatId, MessageId};

/// Minimum time between `editMessageText` calls (Telegram per-chat limit is
/// ~1 msg/s; 3 s keeps us far below it while still feeling live).
const EDIT_INTERVAL: Duration = Duration::from_secs(3);

/// Pure throttle decision, extracted for tests.
fn should_edit(last_edit: Option<Instant>, now: Instant, text_changed: bool, force: bool) -> bool {
    if force {
        return true;
    }
    if !text_changed {
        return false;
    }
    last_edit.is_none_or(|t| now.duration_since(t) >= EDIT_INTERVAL)
}

/// Throttled progress editor: at most one `editMessageText` per [`EDIT_INTERVAL`].
/// The final 100% update always goes through (`force=true`).
pub struct Progress {
    bot: Bot,
    chat: ChatId,
    message: MessageId,
    last_edit: Option<Instant>,
    last_text: String,
}

impl Progress {
    pub fn new(bot: Bot, chat: ChatId, message: MessageId) -> Self {
        Self {
            bot,
            chat,
            message,
            last_edit: None,
            last_text: String::new(),
        }
    }

    /// Edit the message if `text` changed and the throttle window elapsed.
    /// Set `force=true` for the final update.
    pub async fn update(&mut self, text: &str, force: bool) {
        if !should_edit(
            self.last_edit,
            Instant::now(),
            text != self.last_text,
            force,
        ) {
            return;
        }
        if self
            .bot
            .edit_message_text(self.chat, self.message, text.to_owned())
            .await
            .is_ok()
        {
            self.last_edit = Some(Instant::now());
            text.clone_into(&mut self.last_text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_edit_always_allowed() {
        assert!(should_edit(None, Instant::now(), true, false));
    }

    #[test]
    fn unchanged_text_suppressed_unless_forced() {
        let now = Instant::now();
        assert!(!should_edit(None, now, false, false));
        assert!(should_edit(None, now, false, true));
    }

    #[test]
    fn throttle_window_respected() {
        let base = Instant::now();
        let recent = base.checked_sub(Duration::from_secs(1));
        let old = base.checked_sub(Duration::from_secs(4));
        assert!(!should_edit(recent, base, true, false));
        assert!(should_edit(old, base, true, false));
        // Forced final update bypasses the window.
        assert!(should_edit(recent, base, true, true));
    }
}
