use efeengine_core::prelude::*;

#[main]
fn main(){
    match Engine::new("hello") {
        Ok(mut w) => {
            w.state.world.add_object([0.0,0.]);
            global_info("sa");
            w.game_loop();
        }
        Err(e) => eprintln!("engine init failed: {e}"),
    }
    if cfg!(target_os = "windows"){}
}