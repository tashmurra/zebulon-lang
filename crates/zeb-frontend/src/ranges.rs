//! Conservative integer bounds on successful scalar operations.
//! Region-entry slots and call results are unknown; no branch/loop induction proof.
use crate::{
    Diagnostic,
    ir::{Binary, Function, Operation, Unary},
    sema::Scalar,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Range {
    low: i64,
    high: i64,
}
impl Range {
    pub(crate) const FULL: Self = Self {
        low: i32::MIN as i64,
        high: i32::MAX as i64,
    };
    fn singleton(value: i32) -> Self {
        Self {
            low: i64::from(value),
            high: i64::from(value),
        }
    }
    fn checked(low: i64, high: i64) -> Option<Self> {
        (low >= Self::FULL.low && high <= Self::FULL.high).then_some(Self { low, high })
    }
    fn arithmetic(self, op: Binary, rhs: Self) -> Option<Self> {
        match op {
            Binary::Add => Self::checked(self.low + rhs.low, self.high + rhs.high),
            Binary::Subtract => Self::checked(self.low - rhs.high, self.high - rhs.low),
            Binary::Multiply => {
                // All endpoints are signed i32: every product fits signed i64.
                let values = [
                    self.low * rhs.low,
                    self.low * rhs.high,
                    self.high * rhs.low,
                    self.high * rhs.high,
                ];
                Self::checked(*values.iter().min().unwrap(), *values.iter().max().unwrap())
            }
            _ => None,
        }
    }
    pub(crate) fn arithmetic_safe(self, op: Binary, rhs: Self) -> bool {
        self.arithmetic(op, rhs).is_some()
    }
    pub(crate) fn negation_safe(self) -> bool {
        self.low > Self::FULL.low
    }
    fn unary(self, op: Unary) -> Self {
        match op {
            Unary::Positive => self,
            Unary::Negative => Self::checked(-self.high, -self.low).unwrap_or(Self::FULL),
            Unary::BitNot => Self {
                low: -self.high - 1,
                high: -self.low - 1,
            },
            Unary::Not => Self::FULL,
        }
    }
    fn binary(self, op: Binary, rhs: Self) -> Self {
        if let Some(range) = self.arithmetic(op, rhs) {
            return range;
        }
        if op == Binary::And {
            // A nonnegative constant mask bounds any successful integer operand.
            for mask in [self, rhs] {
                if mask.low == mask.high && mask.low >= 0 {
                    return Self {
                        low: 0,
                        high: mask.high,
                    };
                }
            }
        }
        Self::FULL
    }
}
fn full(size: usize) -> Result<Vec<Range>, Diagnostic> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(size)
        .map_err(|_| Diagnostic::resource(0))?;
    result.resize(size, Range::FULL);
    Ok(result)
}
/// Caller must have verified the IR. Bounds assume the operation succeeded;
/// they never discharge type checks or imply that a failing path is unreachable.
pub(crate) fn analyze(function: &Function) -> Result<Vec<Range>, Diagnostic> {
    let mut values = full(function.values)?;
    let mut slots = full(function.slots.len())?;
    for block in &function.blocks {
        // Never carry an assignment across a join/backedge without a CFG proof.
        slots.fill(Range::FULL);
        for instruction in &block.instructions {
            let range = match &instruction.operation {
                Operation::Constant(Scalar::Integer(value)) => Range::singleton(*value),
                Operation::Load(slot) => slots[slot.0],
                Operation::Store(slot, value) => {
                    slots[slot.0] = values[value.0];
                    Range::FULL
                }
                Operation::Reset(slot) => {
                    slots[slot.0] = Range::FULL;
                    Range::FULL
                }
                Operation::Unary(op, value) => values[value.0].unary(*op),
                Operation::Binary(op, left, right) => values[left.0].binary(*op, values[right.0]),
                Operation::Call { .. }
                | Operation::CallMethod { .. }
                | Operation::GetProperty(..) => {
                    slots.fill(Range::FULL);
                    Range::FULL
                }
                _ => Range::FULL,
            };
            if let Some(value) = instruction.result {
                values[value.0] = range;
            }
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interval_claims_contain_independent_boundary_samples() {
        let points = [i32::MIN, -65536, -1, 0, 1, 127, 32767, 65536, i32::MAX];
        for &lo in &points {
            for &hi in points.iter().filter(|&&x| x >= lo) {
                let left = Range {
                    low: i64::from(lo),
                    high: i64::from(hi),
                };
                for &rl in &points {
                    for &rh in points.iter().filter(|&&x| x >= rl) {
                        let right = Range {
                            low: i64::from(rl),
                            high: i64::from(rh),
                        };
                        for op in [Binary::Add, Binary::Subtract, Binary::Multiply] {
                            if let Some(claim) = left.arithmetic(op, right) {
                                for &a in points.iter().filter(|&&x| lo <= x && x <= hi) {
                                    for &b in points.iter().filter(|&&x| rl <= x && x <= rh) {
                                        let (a, b) = (i64::from(a), i64::from(b));
                                        let result = match op {
                                            Binary::Add => a + b,
                                            Binary::Subtract => a - b,
                                            Binary::Multiply => a * b,
                                            _ => unreachable!(),
                                        };
                                        assert!(i32::try_from(result).is_ok());
                                        assert!(claim.low <= result && result <= claim.high);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
