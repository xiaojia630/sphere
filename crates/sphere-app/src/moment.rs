//! The Agent moment card: the first open request, centred over the core.
use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct Moment {
    /// The card is up and has the keyboard.
    pub shown: bool,
    /// Typing a `change` answer.
    pub editing: bool,
}
