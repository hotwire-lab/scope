mod app;
mod components;
mod dsp;
mod protocol;
mod transport;

use app::App;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
