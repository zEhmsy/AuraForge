//! League of Legends game events from Riot's official Live Client Data API.
//!
//! The game client itself serves `https://127.0.0.1:2999/liveclientdata/...`
//! while a match is running; this module only issues plain GET requests to it.
//! Nothing touches the game process, its memory or its input. The endpoint
//! uses a certificate issued by Riot's private root, so certificate checks are
//! relaxed for this one loopback address only.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
    WinHttpSetTimeouts, SECURITY_FLAG_IGNORE_CERT_CN_INVALID, SECURITY_FLAG_IGNORE_CERT_DATE_INVALID,
    SECURITY_FLAG_IGNORE_UNKNOWN_CA, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_SECURITY_FLAGS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};

const PORT: u16 = 2999;
const IN_GAME_POLL: Duration = Duration::from_millis(500);
const IDLE_POLL: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Kill,
    Assist,
    Death,
    /// Kill streak of 2 (double) up to 5 (penta).
    Multikill(u8),
    FirstBlood,
    Ace { ally: bool },
    Dragon { element: String, ally: bool },
    Herald { ally: bool },
    Baron { ally: bool },
    Turret { ally: bool },
    Inhibitor { ally: bool },
    Victory,
    Defeat,
    /// The player died (`true`) or respawned (`false`).
    Dead(bool),
}

/// Owned WinHTTP handle.
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

struct Client {
    _session: Handle,
    connection: Handle,
}

impl Client {
    fn new() -> Result<Self, String> {
        let session = Handle(unsafe {
            WinHttpOpen(w!("AuraForge"), WINHTTP_ACCESS_TYPE_NO_PROXY, PCWSTR::null(), PCWSTR::null(), 0)
        });
        if session.0.is_null() {
            return Err("WinHttpOpen failed".into());
        }
        // The server is local; anything slower than this means no game.
        unsafe { WinHttpSetTimeouts(session.0, 500, 500, 500, 1500) }.map_err(|e| e.message())?;
        let connection = Handle(unsafe { WinHttpConnect(session.0, w!("127.0.0.1"), PORT, 0) });
        if connection.0.is_null() {
            return Err("WinHttpConnect failed".into());
        }
        Ok(Client { _session: session, connection })
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let request = Handle(unsafe {
            WinHttpOpenRequest(
                self.connection.0,
                w!("GET"),
                PCWSTR(wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        });
        if request.0.is_null() {
            return Err("WinHttpOpenRequest failed".into());
        }
        let flags = SECURITY_FLAG_IGNORE_UNKNOWN_CA
            | SECURITY_FLAG_IGNORE_CERT_CN_INVALID
            | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID;
        unsafe {
            WinHttpSetOption(Some(request.0), WINHTTP_OPTION_SECURITY_FLAGS, Some(&flags.to_ne_bytes()))
                .map_err(|e| e.message())?;
            WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|e| e.message())?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| e.message())?;
        }

        let mut status = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(&mut status as *mut u32 as *mut _),
                &mut size,
                std::ptr::null_mut(),
            )
            .map_err(|e| e.message())?;
        }
        if status != 200 {
            return Err(format!("{path}: HTTP {status}"));
        }

        let mut body = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let mut read = 0u32;
            unsafe {
                WinHttpReadData(request.0, chunk.as_mut_ptr().cast(), chunk.len() as u32, &mut read)
                    .map_err(|e| e.message())?;
            }
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
        }
        serde_json::from_slice(&body).map_err(|e| format!("{path}: {e}"))
    }
}

/// Who the player is and which team everybody is on, from `playerlist`.
#[derive(Default)]
struct Roster {
    /// Every spelling the API may use for the player: Riot ID, game name, summoner name.
    me: HashSet<String>,
    my_team: Option<String>,
    teams: HashMap<String, String>,
    dead: bool,
}

impl Roster {
    fn new(active_player: &str, players: &Value) -> Self {
        let mut roster = Roster::default();
        roster.me.insert(active_player.to_owned());
        for p in players.as_array().into_iter().flatten() {
            let names: Vec<&str> = ["riotId", "riotIdGameName", "summonerName"]
                .iter()
                .filter_map(|k| p[k].as_str())
                .filter(|n| !n.is_empty())
                .collect();
            let team = p["team"].as_str().unwrap_or_default();
            for n in &names {
                roster.teams.insert((*n).to_owned(), team.to_owned());
            }
            if names.iter().any(|n| *n == active_player) {
                roster.me.extend(names.iter().map(|n| (*n).to_owned()));
                roster.my_team = Some(team.to_owned());
                roster.dead = p["isDead"].as_bool().unwrap_or(false);
            }
        }
        // The Riot ID is `name#tag`; events may carry only the name.
        if let Some((name, _)) = active_player.split_once('#') {
            roster.me.insert(name.to_owned());
        }
        roster
    }

    fn is_me(&self, name: Option<&str>) -> bool {
        name.is_some_and(|n| self.me.contains(n))
    }

    fn is_ally(&self, name: Option<&str>) -> bool {
        let team = name.and_then(|n| self.teams.get(n));
        team.is_some() && team == self.my_team.as_ref()
    }

    /// Structures are named `Turret_TOrder_...` (older patches: `Turret_T1_...`,
    /// `Barracks_T2_...`; T1 is ORDER). Destroying one is good news when it
    /// belongs to the other team.
    fn is_enemy_structure(&self, name: Option<&str>) -> bool {
        let name = name.unwrap_or_default().to_ascii_lowercase();
        let owner = if name.contains("_t1_") || name.contains("_torder_") {
            "ORDER"
        } else if name.contains("_t2_") || name.contains("_tchaos_") {
            "CHAOS"
        } else {
            return false;
        };
        self.my_team.as_deref().is_some_and(|t| t != owner)
    }
}

/// Maps one `eventdata` entry to the effects it should trigger.
fn translate(event: &Value, roster: &Roster) -> Vec<Event> {
    let s = |k: &str| event[k].as_str();
    let mut out = Vec::new();
    match s("EventName").unwrap_or_default() {
        "ChampionKill" => {
            if roster.is_me(s("KillerName")) {
                out.push(Event::Kill);
            }
            if roster.is_me(s("VictimName")) {
                out.push(Event::Death);
            }
            let assisted = event["Assisters"]
                .as_array()
                .is_some_and(|a| a.iter().any(|n| roster.is_me(n.as_str())));
            if assisted {
                out.push(Event::Assist);
            }
        }
        "Multikill" if roster.is_me(s("KillerName")) => {
            let streak = event["KillStreak"].as_u64().unwrap_or(2).clamp(2, 5) as u8;
            out.push(Event::Multikill(streak));
        }
        "FirstBlood" if roster.is_me(s("Recipient")) => out.push(Event::FirstBlood),
        "Ace" => {
            let ally = s("AcingTeam").is_some() && s("AcingTeam") == roster.my_team.as_deref();
            out.push(Event::Ace { ally });
        }
        "DragonKill" => out.push(Event::Dragon {
            element: s("DragonType").unwrap_or_default().to_owned(),
            ally: roster.is_ally(s("KillerName")),
        }),
        "HeraldKill" => out.push(Event::Herald { ally: roster.is_ally(s("KillerName")) }),
        "BaronKill" => out.push(Event::Baron { ally: roster.is_ally(s("KillerName")) }),
        "TurretKilled" => out.push(Event::Turret { ally: roster.is_enemy_structure(s("TurretKilled")) }),
        "InhibKilled" => out.push(Event::Inhibitor { ally: roster.is_enemy_structure(s("InhibKilled")) }),
        "GameEnd" => out.push(if s("Result") == Some("Win") { Event::Victory } else { Event::Defeat }),
        _ => {}
    }
    out
}

/// Highest `EventID` in an `eventdata` reply, if any.
fn last_event_id(events: &Value) -> Option<u64> {
    events["Events"].as_array()?.iter().filter_map(|e| e["EventID"].as_u64()).max()
}

struct Game {
    roster: Roster,
    /// First `EventID` not handled yet.
    next_event: u64,
}

/// Polls the API while a match is running and forwards translated events.
pub fn run(tx: Sender<Event>, stop: &AtomicBool) {
    let client = match Client::new() {
        Ok(c) => c,
        Err(e) => {
            log!("League events disabled: {e}");
            return;
        }
    };
    let mut game: Option<Game> = None;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(if game.is_some() { IN_GAME_POLL } else { IDLE_POLL });
        match poll(&client, &mut game) {
            Ok(events) => {
                for e in events {
                    if tx.send(e).is_err() {
                        return;
                    }
                }
            }
            Err(e) => {
                if let Some(g) = game.take() {
                    log!("League game left ({e})");
                    if g.roster.dead && tx.send(Event::Dead(false)).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

fn poll(client: &Client, game: &mut Option<Game>) -> Result<Vec<Event>, String> {
    let active = client.get("/liveclientdata/activeplayername")?;
    let active = active.as_str().ok_or("activeplayername is not a string")?;
    let players = client.get("/liveclientdata/playerlist")?;
    let events = client.get("/liveclientdata/eventdata")?;
    let roster = Roster::new(active, &players);

    let Some(g) = game.as_mut() else {
        // Joining a running match: do not replay what already happened.
        let next_event = last_event_id(&events).map_or(0, |id| id + 1);
        log!(
            "League game detected: playing as {active}, team {}, aliases {:?}",
            roster.my_team.as_deref().unwrap_or("?"),
            roster.me
        );
        let dead = roster.dead;
        *game = Some(Game { roster, next_event });
        return Ok(if dead { vec![Event::Dead(true)] } else { Vec::new() });
    };

    let mut out = Vec::new();
    for e in events["Events"].as_array().into_iter().flatten() {
        let Some(id) = e["EventID"].as_u64() else { continue };
        if id < g.next_event {
            continue;
        }
        g.next_event = id + 1;
        let translated = translate(e, &roster);
        log!("event {id} {} -> {translated:?}", e["EventName"].as_str().unwrap_or("?"));
        out.extend(translated);
    }
    if roster.dead != g.roster.dead {
        out.push(Event::Dead(roster.dead));
    }
    g.roster = roster;
    Ok(out)
}

/// Fires every effect in turn, for trying them out without a match.
pub fn demo(tx: Sender<Event>, stop: &AtomicBool) {
    let sequence = [
        Event::Kill,
        Event::Assist,
        Event::Multikill(2),
        Event::Multikill(3),
        Event::Multikill(4),
        Event::Multikill(5),
        Event::FirstBlood,
        Event::Death,
        Event::Dead(true),
        Event::Dead(false),
        Event::Ace { ally: true },
        Event::Ace { ally: false },
        Event::Dragon { element: "Fire".into(), ally: true },
        Event::Dragon { element: "Water".into(), ally: true },
        Event::Dragon { element: "Earth".into(), ally: true },
        Event::Dragon { element: "Air".into(), ally: true },
        Event::Dragon { element: "Hextech".into(), ally: true },
        Event::Dragon { element: "Chemtech".into(), ally: true },
        Event::Dragon { element: "Elder".into(), ally: true },
        Event::Dragon { element: "Fire".into(), ally: false },
        Event::Herald { ally: true },
        Event::Baron { ally: true },
        Event::Baron { ally: false },
        Event::Turret { ally: true },
        Event::Turret { ally: false },
        Event::Inhibitor { ally: true },
        Event::Victory,
        Event::Defeat,
    ];
    for event in sequence.iter().cycle() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        log!("demo: {event:?}");
        if tx.send(event.clone()).is_err() {
            return;
        }
        thread::sleep(Duration::from_secs(if *event == Event::Dead(true) { 5 } else { 6 }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roster() -> Roster {
        let players = json!([
            { "riotId": "Me#EUW", "riotIdGameName": "Me", "summonerName": "Me#EUW", "team": "ORDER", "isDead": false },
            { "riotId": "Mate#EUW", "riotIdGameName": "Mate", "summonerName": "Mate#EUW", "team": "ORDER", "isDead": false },
            { "riotId": "Foe#KR1", "riotIdGameName": "Foe", "summonerName": "Foe#KR1", "team": "CHAOS", "isDead": true },
            { "riotId": "Sona#BOT", "riotIdGameName": "Sona", "summonerName": "Bot Sona", "team": "CHAOS", "isDead": false },
        ]);
        Roster::new("Me#EUW", &players)
    }

    #[test]
    fn kills_deaths_and_assists() {
        let r = roster();
        let kill = json!({ "EventName": "ChampionKill", "KillerName": "Me", "VictimName": "Foe", "Assisters": [] });
        assert_eq!(translate(&kill, &r), [Event::Kill]);
        let death = json!({ "EventName": "ChampionKill", "KillerName": "Foe", "VictimName": "Me#EUW", "Assisters": [] });
        assert_eq!(translate(&death, &r), [Event::Death]);
        let assist = json!({ "EventName": "ChampionKill", "KillerName": "Mate", "VictimName": "Foe", "Assisters": ["Me"] });
        assert_eq!(translate(&assist, &r), [Event::Assist]);
        let other = json!({ "EventName": "ChampionKill", "KillerName": "Foe", "VictimName": "Mate", "Assisters": [] });
        assert!(translate(&other, &r).is_empty());
        // Real practice-tool reply: victims are named by `summonerName`.
        let bot = json!({ "EventName": "ChampionKill", "KillerName": "Me#EUW", "VictimName": "Bot Sona", "Assisters": [] });
        assert_eq!(translate(&bot, &r), [Event::Kill]);
        let penta = json!({ "EventName": "Multikill", "KillerName": "Me", "KillStreak": 5 });
        assert_eq!(translate(&penta, &r), [Event::Multikill(5)]);
    }

    #[test]
    fn objectives_know_the_side() {
        let r = roster();
        let drake = json!({ "EventName": "DragonKill", "KillerName": "Mate", "DragonType": "Fire" });
        assert_eq!(translate(&drake, &r), [Event::Dragon { element: "Fire".into(), ally: true }]);
        let baron = json!({ "EventName": "BaronKill", "KillerName": "Foe" });
        assert_eq!(translate(&baron, &r), [Event::Baron { ally: false }]);
        let ours = json!({ "EventName": "TurretKilled", "TurretKilled": "Turret_T1_L_03_A", "KillerName": "Minion_T200L0S01N0004" });
        assert_eq!(translate(&ours, &r), [Event::Turret { ally: false }]);
        let modern = json!({ "EventName": "TurretKilled", "TurretKilled": "Turret_TChaos_L0_P3_3795288474_0", "KillerName": "Me" });
        assert_eq!(translate(&modern, &r), [Event::Turret { ally: true }]);
        let theirs = json!({ "EventName": "InhibKilled", "InhibKilled": "Barracks_T2_R1", "KillerName": "Mate" });
        assert_eq!(translate(&theirs, &r), [Event::Inhibitor { ally: true }]);
        let ace = json!({ "EventName": "Ace", "Acer": "Mate", "AcingTeam": "ORDER" });
        assert_eq!(translate(&ace, &r), [Event::Ace { ally: true }]);
        let end = json!({ "EventName": "GameEnd", "Result": "Win" });
        assert_eq!(translate(&end, &r), [Event::Victory]);
    }
}
