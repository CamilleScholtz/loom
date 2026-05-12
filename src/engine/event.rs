use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::world::{FactId, LocationId, MemorableEvent, NpcId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Time {
    pub day: u32,
    pub minute: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub kind: String,
    pub summary: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Distortion {
    pub level: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeDelta {
    pub affection: i32,
    pub trust: i32,
    pub debt: i32,
    pub grievance: i32,
    #[serde(default)]
    pub attraction: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cause {
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub kind: String,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Achieved,
    Abandoned,
    Frustrated,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Event {
    Witnessed {
        observer: NpcId,
        fact: FactId,
        when: Time,
    },
    Told {
        teller: NpcId,
        listener: NpcId,
        fact: FactId,
        distortion: Distortion,
        when: Time,
    },
    RelationshipShift {
        subject: NpcId,
        object: NpcId,
        delta: EdgeDelta,
        cause: Cause,
        when: Time,
    },
    GoalFormed {
        holder: NpcId,
        goal: Goal,
        when: Time,
    },
    GoalResolved {
        holder: NpcId,
        goal: Goal,
        outcome: Outcome,
        when: Time,
    },
    Spoken {
        speaker: NpcId,
        listeners: Vec<NpcId>,
        line: String,
        when: Time,
    },
    Moved {
        who: NpcId,
        to: LocationId,
        when: Time,
    },
    Born {
        who: NpcId,
        when: Time,
    },
    Died {
        who: NpcId,
        cause: String,
        when: Time,
    },
    /// Append a memorable event to `holder`'s in-memory `memorable_events` log.
    /// Emitted by the interpretation layer when a player utterance lands in a
    /// way the NPC would notice and remember (insult, comfort, flirt, etc.),
    /// so the npc_voice prompt can surface "memories of you" next turn.
    Remembered {
        holder: NpcId,
        event: MemorableEvent,
        when: Time,
    },
    /// `giver` transfers an item (a plain string) to `receiver`. If the
    /// giver's `inventory` lists the item it is removed; the receiver's
    /// inventory always grows. Emitted by the NPC dialogue tool `hand_over`.
    HandedOver {
        giver: NpcId,
        receiver: NpcId,
        item: String,
        when: Time,
    },
    /// `promisor` has voiced a commitment to `promisee`. The simulation
    /// records this on the promisor's `promises` list; future ticks can
    /// score honored/broken outcomes. Emitted by the NPC dialogue tool
    /// `promise`.
    PromiseMade {
        promisor: NpcId,
        promisee: NpcId,
        summary: String,
        #[serde(default)]
        by_day: Option<u32>,
        when: Time,
    },
    /// A new location has been named into existence during dialogue. The
    /// engine allocated `id` and linked it bidirectionally to `adjacent_to`;
    /// the LLM only supplied the surface text (`name`, `kind`, `description`).
    /// Emitted by the NPC dialogue tool `discover_location` — typically
    /// when an NPC says "let's go to my bunk" and "bunk" isn't a modeled
    /// location yet.
    LocationDiscovered {
        id: LocationId,
        name: String,
        /// Optional category for the new place ("bunk", "corridor"). Field
        /// is renamed at the wire so it doesn't clash with the enum's
        /// `tag = "kind"` discriminator.
        #[serde(default, rename = "location_kind")]
        location_kind: String,
        #[serde(default)]
        description: String,
        adjacent_to: LocationId,
        when: Time,
    },
}

pub struct EventLog {
    path: PathBuf,
    writer: BufWriter<File>,
}

impl EventLog {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening event log at {}", path.display()))?;
        Ok(Self {
            path,
            writer: BufWriter::new(file),
        })
    }

    pub fn append(&mut self, event: &Event) -> Result<()> {
        serde_json::to_writer(&mut self.writer, event)
            .with_context(|| "serializing event to log")?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read_all(path: impl AsRef<Path>) -> Result<Vec<Event>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(path)
            .with_context(|| format!("opening event log at {}", path.display()))?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for (line_no, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let event: Event = serde_json::from_str(&line).with_context(|| {
                format!("parsing event log line {} of {}", line_no + 1, path.display())
            })?;
            events.push(event);
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_events() -> Vec<Event> {
        let when = Time { day: 1, minute: 480 };
        vec![
            Event::Witnessed {
                observer: NpcId(1),
                fact: FactId(1),
                when,
            },
            Event::Told {
                teller: NpcId(1),
                listener: NpcId(2),
                fact: FactId(2),
                distortion: Distortion { level: 1 },
                when,
            },
            Event::RelationshipShift {
                subject: NpcId(1),
                object: NpcId(2),
                delta: EdgeDelta {
                    affection: -2,
                    trust: -1,
                    debt: 0,
                    grievance: 1,
                    attraction: 0,
                },
                cause: Cause {
                    summary: "broken promise".into(),
                },
                when,
            },
            Event::GoalFormed {
                holder: NpcId(2),
                goal: Goal {
                    kind: "revenge".into(),
                    summary: "even the score with the miller".into(),
                },
                when,
            },
            Event::GoalResolved {
                holder: NpcId(2),
                goal: Goal {
                    kind: "revenge".into(),
                    summary: "even the score with the miller".into(),
                },
                outcome: Outcome::Abandoned,
                when,
            },
            Event::Spoken {
                speaker: NpcId(1),
                listeners: vec![NpcId(2), NpcId(3)],
                line: "Good morrow to you.".into(),
                when,
            },
            Event::Moved {
                who: NpcId(1),
                to: LocationId(7),
                when,
            },
        ]
    }

    #[test]
    fn event_log_round_trips_jsonl() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.log");
        let events = sample_events();

        {
            let mut log = EventLog::open(&path).unwrap();
            for e in &events {
                log.append(e).unwrap();
            }
        }

        let back = EventLog::read_all(&path).unwrap();
        assert_eq!(events, back);
    }

    #[test]
    fn event_log_each_line_parses_alone() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.log");
        let events = sample_events();
        {
            let mut log = EventLog::open(&path).unwrap();
            for e in &events {
                log.append(e).unwrap();
            }
        }

        let raw = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), events.len());
        for (i, line) in lines.iter().enumerate() {
            let parsed: Event = serde_json::from_str(line).unwrap();
            assert_eq!(parsed, events[i]);
        }
    }

    #[test]
    fn read_all_on_missing_path_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nope.log");
        let events = EventLog::read_all(&path).unwrap();
        assert!(events.is_empty());
    }
}
