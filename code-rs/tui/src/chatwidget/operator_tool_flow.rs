use super::*;
use crate::tool_command::{self, ToolCommand};

impl ChatWidget<'_> {
    pub(crate) fn handle_tool_command(&mut self, args: String) {
        match tool_command::parse(&args) {
            Ok(ToolCommand::List) => self.submit_op(Op::ListTools),
            Ok(ToolCommand::Use { name, arguments }) => {
                self.submit_op(Op::InvokeTool { name, arguments });
            }
            Err(message) => self.on_operator_tool_response(message),
        }
    }

    pub(crate) fn on_operator_tool_response(&mut self, message: String) {
        self.history_push_plain_paragraphs(
            crate::history::state::PlainMessageKind::Notice,
            message.lines(),
        );
        self.request_redraw();
    }
}
