use crate::{pty::App, renderer};
use anyhow::Result;

pub fn run(mut app: App) -> Result<()> {
    renderer::render(&mut app)
}
