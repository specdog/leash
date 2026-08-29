use crate::DomainError;

macro_rules! finite_unit {
    ($name:ident, $label:literal) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd)]
        pub struct $name(f64);

        impl $name {
            pub fn new(value: f64) -> Result<Self, DomainError> {
                if !value.is_finite() {
                    return Err(DomainError::NonFinite($label));
                }
                Ok(Self(value))
            }

            pub const fn get(self) -> f64 {
                self.0
            }

            pub fn checked_add(self, rhs: Self) -> Result<Self, DomainError> {
                Self::new(self.0 + rhs.0)
            }

            pub fn checked_sub(self, rhs: Self) -> Result<Self, DomainError> {
                Self::new(self.0 - rhs.0)
            }
        }
    };
}

finite_unit!(Meters, "meters");
finite_unit!(MetersPerSecond, "meters per second");
finite_unit!(Radians, "radians");
finite_unit!(RadiansPerSecond, "radians per second");

/// Units cannot be mixed accidentally.
///
/// ```compile_fail
/// use leash_core::{Meters, Radians};
/// let distance = Meters::new(1.0).unwrap();
/// let angle = Radians::new(1.0).unwrap();
/// let _invalid = distance.checked_add(angle);
/// ```
#[cfg(doctest)]
struct UnitCompileContract;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_reject_non_finite_values() {
        assert_eq!(Meters::new(f64::NAN), Err(DomainError::NonFinite("meters")));
        assert_eq!(
            RadiansPerSecond::new(f64::INFINITY),
            Err(DomainError::NonFinite("radians per second"))
        );
    }
}
