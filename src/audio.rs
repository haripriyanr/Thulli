//! Ambient sound for the Pomodoro timer. The sounds are embedded in the
//! program, so nothing has to be installed next to it. Audio problems are
//! silent by design: the timer keeps working without sound.

use std::io::Cursor;

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

static FOREST: &[u8] = include_bytes!("../assets/sounds/forest.mp3");
static LOFI: &[u8] = include_bytes!("../assets/sounds/lofi-quickie-trix.mp3");
static RAIN: [&[u8]; 4] = [
    include_bytes!("../assets/sounds/rain-1.mp3"),
    include_bytes!("../assets/sounds/rain-2.mp3"),
    include_bytes!("../assets/sounds/rain-3.mp3"),
    include_bytes!("../assets/sounds/rain-4.mp3"),
];

/// The ambient choices offered on the Pomodoro screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ambient {
    Off,
    Forest,
    LoFi,
    Rain,
}

impl Ambient {
    /// Stable identifier used in preferences and at the UI boundary.
    pub fn as_str(self) -> &'static str {
        match self {
            Ambient::Off => "off",
            Ambient::Forest => "forest",
            Ambient::LoFi => "lofi",
            Ambient::Rain => "rain",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "off" => Some(Ambient::Off),
            "forest" => Some(Ambient::Forest),
            "lofi" => Some(Ambient::LoFi),
            "rain" => Some(Ambient::Rain),
            _ => None,
        }
    }
}

/// Plays one ambient sound at a time, in step with the timer.
#[derive(Default)]
pub struct AmbientPlayer {
    // The device must stay alive for as long as the player plays.
    sink: Option<MixerDeviceSink>,
    player: Option<Player>,
    current: Option<Ambient>,
}

impl AmbientPlayer {
    /// Makes `choice` the sound to play, and plays it only while `playing` is
    /// true. Called whenever the timer changes or ticks.
    pub fn sync(&mut self, choice: Ambient, playing: bool) {
        if self.current != Some(choice) {
            self.load(choice);
        }
        let Some(player) = &self.player else {
            return;
        };
        if playing && choice != Ambient::Off {
            // Rain is a short list of recordings, so it is queued again once
            // the list has played out.
            if choice == Ambient::Rain && player.empty() {
                queue(player, choice);
            }
            player.play();
        } else {
            player.pause();
        }
    }

    fn load(&mut self, choice: Ambient) {
        self.player = None;
        self.current = Some(choice);
        if choice == Ambient::Off {
            return;
        }
        if self.sink.is_none() {
            self.sink = DeviceSinkBuilder::open_default_sink().ok();
        }
        let Some(sink) = &self.sink else {
            return;
        };
        let player = Player::connect_new(sink.mixer());
        player.pause();
        queue(&player, choice);
        self.player = Some(player);
    }
}

/// Queues the recording for `choice`. Forest and Lo-fi loop forever. Rain
/// queues its recordings in order.
fn queue(player: &Player, choice: Ambient) {
    match choice {
        Ambient::Forest => {
            if let Ok(source) = Decoder::new_looped(Cursor::new(FOREST)) {
                player.append(source);
            }
        }
        Ambient::LoFi => {
            if let Ok(source) = Decoder::new_looped(Cursor::new(LOFI)) {
                player.append(source);
            }
        }
        Ambient::Rain => {
            for bytes in RAIN {
                if let Ok(source) = Decoder::new(Cursor::new(bytes)) {
                    player.append(source);
                }
            }
        }
        Ambient::Off => {}
    }
}
