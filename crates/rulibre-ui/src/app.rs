use iced::widget::{container, text};
use iced::{Element, Fill};

#[derive(Default)]
pub struct State {}

#[derive(Debug, Clone)]
pub enum Message {}

pub fn update(_state: &mut State, message: Message) {
    match message {}
}

pub fn view(_state: &State) -> Element<'_, Message> {
    container(text("Rulibre").size(24))
        .center_x(Fill)
        .center_y(Fill)
        .into()
}
