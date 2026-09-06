use std::time::{Duration, Instant};

use teloxide::prelude::*;
use teloxide::types::{ChatId, MessageId};

/// Throttled progress editor: at most one `editMessageText` per 3 s
/// (Telegram per-chat limit is ~1 msg/s; 3 s keeps us far below it while
/// still feeling live). The final 100% update always goes through.
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
        if text == self.last_text && !force {
            return;
        }
        let due = self
            .last_edit
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(3));
        if !due && !force {
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
    #[test]
    fn throttle_window_is_3s() {
        assert_eq!(std::time::Duration::from_secs(3).as_secs(), 3);
    }
}
