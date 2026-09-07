//! Independent recomputation of a retained compute trace.
//!
//! This is the controller-side twin of `eval/src/proof_eval/compute_trace.py`.
//! A supplied `flops_used` is never trusted; only the op list is. Unlisted
//! compute-shaped ops refuse. A one-matmul fixture does not attest every recipe.

use serde::Deserialize;

use crate::ResearchError;

const TRACE_SCHEMA: u32 = 1;

/// One compacted op from a retained trace.
#[derive(Debug, Clone, Deserialize)]
pub struct ComputeOp {
    pub op: String,
    pub shapes: Vec<Vec<i64>>,
    pub dtype: String,
    pub count: u64,
}

/// Retained op list. The `flops_used` field, if present, is ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct ComputeTrace {
    pub schema_version: u32,
    pub ops: Vec<ComputeOp>,
}

/// Recompute FLOPs from a retained trace. Never invents a number.
///
/// # Errors
/// Empty/malformed trace, unlisted compute op, overflow, or no listed work.
pub fn flop_total(trace: &ComputeTrace) -> Result<u64, ResearchError> {
    if trace.schema_version != TRACE_SCHEMA || trace.ops.is_empty() {
        return Err(ResearchError::Evidence);
    }
    let mut total: u64 = 0;
    let mut saw_compute = false;
    for op in &trace.ops {
        if op.dtype.trim().is_empty() || op.count == 0 {
            return Err(ResearchError::Evidence);
        }
        let name = canonical_op(&op.op);
        if looks_compute(&name) && !listed(&name) {
            return Err(ResearchError::Evidence);
        }
        let flops = op_flops(&name, &op.shapes)?
            .checked_mul(op.count)
            .ok_or(ResearchError::Evidence)?;
        if flops > 0 {
            saw_compute = true;
        }
        total = total.checked_add(flops).ok_or(ResearchError::Evidence)?;
    }
    if !saw_compute {
        return Err(ResearchError::Evidence);
    }
    Ok(total)
}

fn canonical_op(name: &str) -> String {
    let mut raw = name.trim().to_owned();
    if let Some(rest) = raw.strip_prefix("aten.") {
        raw = format!("aten::{rest}");
    }
    if let Some(stripped) = raw
        .strip_suffix(".default")
        .or_else(|| raw.strip_suffix(".out"))
    {
        stripped.to_owned()
    } else {
        raw
    }
}

fn listed(name: &str) -> bool {
    matches!(
        name,
        "aten::mm"
            | "aten::addmm"
            | "aten::bmm"
            | "aten::baddbmm"
            | "aten::scaled_dot_product_attention"
            | "aten::_scaled_dot_product_attention_math"
    )
}

fn looks_compute(name: &str) -> bool {
    listed(name)
        || name.ends_with("::mm")
        || name
            .rsplit_once('.')
            .is_some_and(|(_, suffix)| suffix == "mm")
        || [
            "matmul",
            "addmm",
            "baddbmm",
            "addbmm",
            "convolution",
            "conv2d",
            "conv1d",
            "conv3d",
            "conv_transpose",
            "scaled_dot_product",
            "flash_attention",
            "cudnn_convolution",
            "miopen",
        ]
        .iter()
        .any(|marker| name.contains(marker))
}

fn op_flops(name: &str, shapes: &[Vec<i64>]) -> Result<u64, ResearchError> {
    match name {
        "aten::mm" | "aten::bmm" => {
            let pair = shapes.iter().rev().take(2).collect::<Vec<_>>();
            if pair.len() < 2 {
                return Err(ResearchError::Evidence);
            }
            matmul(pair[1], pair[0])
        }
        "aten::addmm" | "aten::baddbmm" => {
            let pair = shapes.iter().rev().take(2).collect::<Vec<_>>();
            if pair.len() < 2 || shapes.len() < 3 {
                return Err(ResearchError::Evidence);
            }
            matmul(pair[1], pair[0])
        }
        "aten::scaled_dot_product_attention" | "aten::_scaled_dot_product_attention_math" => {
            if shapes.len() < 3 {
                return Err(ResearchError::Evidence);
            }
            let q = &shapes[0];
            let k = &shapes[1];
            let v = &shapes[2];
            if k.len() < 2 || q.len() < 2 {
                return Err(ResearchError::Evidence);
            }
            let mut kt = k[..k.len() - 2].to_vec();
            kt.push(k[k.len() - 1]);
            kt.push(k[k.len() - 2]);
            let mut scores = q[..q.len() - 2].to_vec();
            scores.push(q[q.len() - 2]);
            scores.push(k[k.len() - 2]);
            let qk = matmul(q, &kt)?;
            let av = matmul(&scores, v)?;
            qk.checked_add(av).ok_or(ResearchError::Evidence)
        }
        _ if looks_compute(name) => Err(ResearchError::Evidence),
        _ => Ok(0),
    }
}

fn matmul(left: &[i64], right: &[i64]) -> Result<u64, ResearchError> {
    if left.len() < 2 || right.len() < 2 || left[left.len() - 1] != right[right.len() - 2] {
        return Err(ResearchError::Evidence);
    }
    let left_batch = &left[..left.len() - 2];
    let right_batch = &right[..right.len() - 2];
    let width = left_batch.len().max(right_batch.len());
    let pad = |dims: &[i64], width: usize| -> Vec<i64> {
        let mut out = vec![1; width.saturating_sub(dims.len())];
        out.extend_from_slice(dims);
        out
    };
    let padded_left = pad(left_batch, width);
    let padded_right = pad(right_batch, width);
    let mut batch: u64 = 1;
    for (lhs, rhs) in padded_left.iter().zip(padded_right.iter()) {
        if *lhs != *rhs && *lhs != 1 && *rhs != 1 {
            return Err(ResearchError::Evidence);
        }
        let dim = u64::try_from((*lhs).max(*rhs)).map_err(|_| ResearchError::Evidence)?;
        batch = batch.checked_mul(dim).ok_or(ResearchError::Evidence)?;
    }
    let rows = u64::try_from(left[left.len() - 2]).map_err(|_| ResearchError::Evidence)?;
    let inner = u64::try_from(left[left.len() - 1]).map_err(|_| ResearchError::Evidence)?;
    let cols = u64::try_from(right[right.len() - 1]).map_err(|_| ResearchError::Evidence)?;
    batch
        .checked_mul(2)
        .and_then(|v| v.checked_mul(rows))
        .and_then(|v| v.checked_mul(inner))
        .and_then(|v| v.checked_mul(cols))
        .ok_or(ResearchError::Evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(name: &str, shapes: &[&[i64]], count: u64) -> ComputeOp {
        ComputeOp {
            op: name.into(),
            shapes: shapes.iter().map(|s| s.to_vec()).collect(),
            dtype: "float32".into(),
            count,
        }
    }

    #[test]
    fn independent_recompute_matches_the_published_mm_cost() {
        let trace = ComputeTrace {
            schema_version: 1,
            ops: vec![op("aten.mm.default", &[&[2, 3], &[3, 4]], 1)],
        };
        assert_eq!(flop_total(&trace).expect("mm"), 48);
    }

    #[test]
    fn unlisted_compute_refuses_instead_of_undercounting() {
        let trace = ComputeTrace {
            schema_version: 1,
            ops: vec![op("aten::convolution", &[&[1, 1, 3, 3], &[1, 1, 1, 1]], 1)],
        };
        assert!(flop_total(&trace).is_err());
    }

    #[test]
    fn empty_or_zero_work_refuses() {
        assert!(flop_total(&ComputeTrace {
            schema_version: 1,
            ops: vec![],
        })
        .is_err());
        let view = ComputeTrace {
            schema_version: 1,
            ops: vec![op("aten::view", &[&[2, 3]], 1)],
        };
        assert!(flop_total(&view).is_err());
    }
}
