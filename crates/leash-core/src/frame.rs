use core::marker::PhantomData;

use crate::{DomainError, Meters, Radians};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameName(Box<str>);

impl FrameName {
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty() {
            return Err(DomainError::Empty("frame name"));
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'/'))
        {
            return Err(DomainError::InvalidCharacter("frame name"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Map {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Odom {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensor {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame<Tag> {
    name: FrameName,
    marker: PhantomData<fn() -> Tag>,
}

impl<Tag> Frame<Tag> {
    pub fn new(name: FrameName) -> Self {
        Self {
            name,
            marker: PhantomData,
        }
    }

    pub fn name(&self) -> &FrameName {
        &self.name
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pose2<Tag> {
    pub frame: Frame<Tag>,
    pub x: Meters,
    pub y: Meters,
    pub yaw: Radians,
}

impl<Tag> Pose2<Tag> {
    pub fn new(frame: Frame<Tag>, x: Meters, y: Meters, yaw: Radians) -> Self {
        Self { frame, x, y, yaw }
    }
}

/// Frame markers prevent map and odometry poses from being exchanged silently.
///
/// ```compile_fail
/// use leash_core::{Frame, FrameName, Map, Meters, Odom, Pose2, Radians};
/// fn consume_map(_: Pose2<Map>) {}
/// let odom = Frame::<Odom>::new(FrameName::new("odom").unwrap());
/// let pose = Pose2::new(
///     odom,
///     Meters::new(0.0).unwrap(),
///     Meters::new(0.0).unwrap(),
///     Radians::new(0.0).unwrap(),
/// );
/// consume_map(pose);
/// ```
#[cfg(doctest)]
struct FrameCompileContract;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_names_are_owned_and_wire_safe() {
        assert_eq!(FrameName::new(""), Err(DomainError::Empty("frame name")));
        assert_eq!(
            FrameName::new("map frame"),
            Err(DomainError::InvalidCharacter("frame name"))
        );
        assert_eq!(
            FrameName::new("robot/base_link").unwrap().as_str(),
            "robot/base_link"
        );
    }
}
