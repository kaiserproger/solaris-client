use super::super::{Entity, Models, presentation as p};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub(crate) struct MethodFact {
    pub owner: String,
    pub parameters: Vec<String>,
}
#[derive(Debug, Deserialize)]
pub(crate) struct MotionFacts {
    pub class: String,
    pub is_mob: bool,
    walk_uses_y: WalkVertical,
    pub methods: BTreeMap<String, MethodFact>,
    pub head_rule: Option<HeadRule>,
    pub body_head_rule: Option<BodyHeadRule>,
    pub body_rule: Option<BodyRule>,
    pub walk_rule: Option<WalkRule>,
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WalkVertical {
    Never,
    Always,
    BodyEquipped,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum HeadRule {
    Fixed {
        #[serde(deserialize_with = "source_f32")]
        value: f32,
    },
    Armadillo {
        #[serde(deserialize_with = "source_f32")]
        idle: f32,
        #[serde(deserialize_with = "source_f32")]
        scared: f32,
    },
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum BodyHeadRule {
    Fixed {
        #[serde(deserialize_with = "source_f32")]
        value: f32,
    },
    Blocking {
        #[serde(deserialize_with = "source_f32")]
        ordinary: f32,
        #[serde(deserialize_with = "source_f32")]
        blocking: f32,
    },
}
// Internally tagged serde enums buffer arbitrary-precision JSON numbers as
// private maps. Recover serde_json's Number before decoding the scalar.
fn source_f32<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    serde_json::from_value(serde_json::Value::deserialize(deserializer)?)
        .map_err(serde::de::Error::custom)
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BodyRule {
    Living,
    Standard,
    Armadillo,
    Camel,
    HappyGhast,
    Phantom,
    Shulker,
    Creaking,
    ArmorStand,
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WalkRule {
    Standard,
    Frog,
    Camel,
    Creaking,
}
impl MotionFacts {
    pub fn walk_uses_y(&self, entity: &Entity) -> bool {
        match self.walk_uses_y {
            WalkVertical::Never => false,
            WalkVertical::Always => true,
            WalkVertical::BodyEquipped => entity.equipment_value(6).is_some_and(|stack| stack.count > 0),
        }
    }
    pub fn max_head_yaw(&self, models: &Models, entity: &Entity) -> Option<f32> {
        self.head_rule.as_ref().map(|rule| match rule {
            HeadRule::Fixed { value } => *value,
            HeadRule::Armadillo { idle, scared } => {
                if p::number(models, entity, "ARMADILLO_STATE").unwrap_or(0) == 0 {
                    *idle
                } else {
                    *scared
                }
            }
        })
    }
    pub fn max_body_head_yaw(&self, is_blocking: bool) -> Option<f32> {
        self.body_head_rule.as_ref().map(|rule| match rule {
            BodyHeadRule::Fixed { value } => *value,
            BodyHeadRule::Blocking { ordinary, blocking } => {
                if is_blocking {
                    *blocking
                } else {
                    *ordinary
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_motion_rules_decode_decimal_limits() {
        let head: HeadRule =
            serde_json::from_str(r#"{"kind":"armadillo","idle":32.5,"scared":0.25}"#).unwrap();
        let HeadRule::Armadillo { idle, scared } = head else {
            panic!("head rule variant")
        };
        assert_eq!([idle, scared], [32.5, 0.25]);
        let body: BodyHeadRule =
            serde_json::from_str(r#"{"kind":"blocking","ordinary":75.5,"blocking":15.25}"#)
                .unwrap();
        let BodyHeadRule::Blocking { ordinary, blocking } = body else {
            panic!("body rule variant")
        };
        assert_eq!([ordinary, blocking], [75.5, 15.25]);
        let fixed: HeadRule = serde_json::from_str(r#"{"kind":"fixed","value":90.0}"#).unwrap();
        assert!(matches!(fixed, HeadRule::Fixed { value: 90. }));
    }
}
