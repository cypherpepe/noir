//! This file is for pretty-printing the SSA IR in a human-readable form for debugging.
use std::{
    collections::HashSet,
    fmt::{Formatter, Result},
};

use acvm::acir::circuit::{ErrorSelector, STRING_ERROR_SELECTOR};
use acvm::acir::AcirField;
use iter_extended::vecmap;

use super::{
    basic_block::BasicBlockId,
    dfg::DataFlowGraph,
    function::Function,
    instruction::{ConstrainError, Instruction, InstructionId, TerminatorInstruction},
    value::{Value, ValueId},
};

/// Helper function for Function's Display impl to pretty-print the function with the given formatter.
pub(crate) fn display_function(function: &Function, f: &mut Formatter) -> Result {
    writeln!(f, "{} fn {} {} {{", function.runtime(), function.name(), function.id())?;
    display_block_with_successors(function, function.entry_block(), &mut HashSet::new(), f)?;
    write!(f, "}}")
}

/// Displays a block followed by all of its successors recursively.
/// This uses a HashSet to keep track of the visited blocks. Otherwise
/// there would be infinite recursion for any loops in the IR.
pub(crate) fn display_block_with_successors(
    function: &Function,
    block_id: BasicBlockId,
    visited: &mut HashSet<BasicBlockId>,
    f: &mut Formatter,
) -> Result {
    display_block(function, block_id, f)?;
    visited.insert(block_id);

    for successor in function.dfg[block_id].successors() {
        if !visited.contains(&successor) {
            display_block_with_successors(function, successor, visited, f)?;
        }
    }
    Ok(())
}

/// Display a single block. This will not display the block's successors.
pub(crate) fn display_block(
    function: &Function,
    block_id: BasicBlockId,
    f: &mut Formatter,
) -> Result {
    let block = &function.dfg[block_id];

    writeln!(f, "  {}({}):", block_id, value_list_with_types(function, block.parameters()))?;

    for instruction in block.instructions() {
        display_instruction(function, *instruction, f)?;
    }

    display_terminator(function, block.terminator(), f)
}

/// Specialize displaying value ids so that if they refer to a numeric
/// constant or a function we print those directly.
fn value(function: &Function, id: ValueId) -> String {
    let id = function.dfg.resolve(id);
    match &function.dfg[id] {
        Value::NumericConstant { constant, typ } => {
            format!("{typ} {constant}")
        }
        Value::Function(id) => id.to_string(),
        Value::Intrinsic(intrinsic) => intrinsic.to_string(),
        Value::Array { array, typ } => {
            let elements = vecmap(array, |element| value(function, *element));
            let element_types = &typ.clone().element_types();
            let element_types_str =
                element_types.iter().map(|typ| typ.to_string()).collect::<Vec<String>>().join(", ");
            if element_types.len() == 1 {
                format!("[{}] of {}", elements.join(", "), element_types_str)
            } else {
                format!("[{}] of ({})", elements.join(", "), element_types_str)
            }
        }
        Value::Param { .. } | Value::Instruction { .. } | Value::ForeignFunction(_) => {
            id.to_string()
        }
    }
}

/// Display each value along with its type. E.g. `v0: Field, v1: u64, v2: u1`
fn value_list_with_types(function: &Function, values: &[ValueId]) -> String {
    vecmap(values, |id| {
        let value = value(function, *id);
        let typ = function.dfg.type_of_value(*id);
        format!("{value}: {typ}")
    })
    .join(", ")
}

/// Display each value separated by a comma
fn value_list(function: &Function, values: &[ValueId]) -> String {
    vecmap(values, |id| value(function, *id)).join(", ")
}

/// Display a terminator instruction
pub(crate) fn display_terminator(
    function: &Function,
    terminator: Option<&TerminatorInstruction>,
    f: &mut Formatter,
) -> Result {
    match terminator {
        Some(TerminatorInstruction::Jmp { destination, arguments, call_stack: _ }) => {
            writeln!(f, "    jmp {}({})", destination, value_list(function, arguments))
        }
        Some(TerminatorInstruction::JmpIf {
            condition,
            then_destination,
            else_destination,
            call_stack: _,
        }) => {
            writeln!(
                f,
                "    jmpif {} then: {}, else: {}",
                value(function, *condition),
                then_destination,
                else_destination
            )
        }
        Some(TerminatorInstruction::Return { return_values, .. }) => {
            if return_values.is_empty() {
                writeln!(f, "    return")
            } else {
                writeln!(f, "    return {}", value_list(function, return_values))
            }
        }
        None => writeln!(f, "    (no terminator instruction)"),
    }
}

/// Display an arbitrary instruction
pub(crate) fn display_instruction(
    function: &Function,
    instruction: InstructionId,
    f: &mut Formatter,
) -> Result {
    // instructions are always indented within a function
    write!(f, "    ")?;

    let results = function.dfg.instruction_results(instruction);
    if !results.is_empty() {
        write!(f, "{} = ", value_list(function, results))?;
    }

    display_instruction_inner(function, &function.dfg[instruction], results, f)
}

fn display_instruction_inner(
    function: &Function,
    instruction: &Instruction,
    results: &[ValueId],
    f: &mut Formatter,
) -> Result {
    let show = |id| value(function, id);

    match instruction {
        Instruction::Binary(binary) => {
            writeln!(f, "{} {}, {}", binary.operator, show(binary.lhs), show(binary.rhs))
        }
        Instruction::Cast(lhs, typ) => writeln!(f, "cast {} as {typ}", show(*lhs)),
        Instruction::Not(rhs) => writeln!(f, "not {}", show(*rhs)),
        Instruction::Truncate { value, bit_size, max_bit_size } => {
            let value = show(*value);
            writeln!(f, "truncate {value} to {bit_size} bits, max_bit_size: {max_bit_size}",)
        }
        Instruction::Constrain(lhs, rhs, error) => {
            write!(f, "constrain {} == {}", show(*lhs), show(*rhs))?;
            if let Some(error) = error {
                display_constrain_error(function, error, f)
            } else {
                writeln!(f)
            }
        }
        Instruction::Call { func, arguments } => {
            let arguments = value_list(function, arguments);
            writeln!(f, "call {}({}){}", show(*func), arguments, result_types(function, results))
        }
        Instruction::Allocate => {
            writeln!(f, "allocate{}", result_types(function, results))
        }
        Instruction::Load { address } => {
            writeln!(f, "load {}{}", show(*address), result_types(function, results))
        }
        Instruction::Store { address, value } => {
            writeln!(f, "store {} at {}", show(*value), show(*address))
        }
        Instruction::EnableSideEffectsIf { condition } => {
            writeln!(f, "enable_side_effects {}", show(*condition))
        }
        Instruction::ArrayGet { array, index } => {
            writeln!(
                f,
                "array_get {}, index {}{}",
                show(*array),
                show(*index),
                result_types(function, results)
            )
        }
        Instruction::ArraySet { array, index, value, mutable } => {
            let array = show(*array);
            let index = show(*index);
            let value = show(*value);
            let mutable = if *mutable { " mut" } else { "" };
            writeln!(f, "array_set{mutable} {array}, index {index}, value {value}")
        }
        Instruction::IncrementRc { value } => {
            writeln!(f, "inc_rc {}", show(*value))
        }
        Instruction::DecrementRc { value } => {
            writeln!(f, "dec_rc {}", show(*value))
        }
        Instruction::RangeCheck { value, max_bit_size, .. } => {
            writeln!(f, "range_check {} to {} bits", show(*value), *max_bit_size,)
        }
        Instruction::IfElse { then_condition, then_value, else_condition, else_value } => {
            let then_condition = show(*then_condition);
            let then_value = show(*then_value);
            let else_condition = show(*else_condition);
            let else_value = show(*else_value);
            writeln!(
                f,
                "if {then_condition} then {then_value} else if {else_condition} then {else_value}"
            )
        }
    }
}

fn result_types(function: &Function, results: &[ValueId]) -> String {
    let types = vecmap(results, |result| function.dfg.type_of_value(*result).to_string());
    if types.is_empty() {
        String::new()
    } else if types.len() == 1 {
        format!(" -> {}", types[0])
    } else {
        format!(" -> ({})", types.join(", "))
    }
}

/// Tries to extract a constant string from an error payload.
pub(crate) fn try_to_extract_string_from_error_payload(
    error_selector: ErrorSelector,
    values: &[ValueId],
    dfg: &DataFlowGraph,
) -> Option<String> {
    ((error_selector == STRING_ERROR_SELECTOR) && (values.len() == 1))
        .then_some(())
        .and_then(|()| {
            let Value::Array { array: values, .. } = &dfg[values[0]] else {
                return None;
            };
            let fields: Option<Vec<_>> =
                values.iter().map(|value_id| dfg.get_numeric_constant(*value_id)).collect();

            fields
        })
        .map(|fields| {
            fields
                .iter()
                .map(|field| {
                    let as_u8 = field.try_to_u64().unwrap_or_default() as u8;
                    as_u8 as char
                })
                .collect()
        })
}

fn display_constrain_error(
    function: &Function,
    error: &ConstrainError,
    f: &mut Formatter,
) -> Result {
    match error {
        ConstrainError::StaticString(assert_message_string) => {
            writeln!(f, " '{assert_message_string:?}'")
        }
        ConstrainError::Dynamic(selector, values) => {
            if let Some(constant_string) =
                try_to_extract_string_from_error_payload(*selector, values, &function.dfg)
            {
                writeln!(f, " '{}'", constant_string)
            } else {
                writeln!(f, ", data {}", value_list(function, values))
            }
        }
    }
}
