use crate::unicorn::{HashableNodeRef, Model, Node, NodeRef, NodeType};
use anyhow::Result;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::rc::Rc;

//
// Public Interface
//

// TODO: MAYBE CX gates are not needed. (We can just push the control qubit to the result)
// BEGIN structs declaration
pub type UnitaryRef = Rc<RefCell<Unitary>>;

#[derive(Debug)]
pub enum Unitary {
    Not {
        input: QubitRef,
    },
    Cnot {
        control: QubitRef,
        target: QubitRef,
    },
    Mcx {
        controls: Vec<QubitRef>,
        target: QubitRef,
    },
    Barrier,
}

impl From<Unitary> for UnitaryRef {
    fn from(unitary: Unitary) -> Self {
        Rc::new(RefCell::new(unitary))
    }
}

#[derive(Debug)]
pub enum Qubit {
    ConstTrue,
    ConstFalse,
    QBit { name: String },
}

impl From<Qubit> for QubitRef {
    fn from(gate: Qubit) -> Self {
        Rc::new(RefCell::new(gate))
    }
}

pub type QubitRef = Rc<RefCell<Qubit>>;

#[derive(Debug)]
pub struct HashableQubitRef {
    pub value: QubitRef,
}

impl Eq for HashableQubitRef {}

impl From<QubitRef> for HashableQubitRef {
    fn from(node: QubitRef) -> Self {
        Self { value: node }
    }
}

impl Hash for HashableQubitRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        RefCell::as_ptr(&self.value).hash(state);
    }
}

impl PartialEq for HashableQubitRef {
    fn eq(&self, other: &Self) -> bool {
        RefCell::as_ptr(&self.value) == RefCell::as_ptr(&other.value)
    }
}

// END structs declaration

// BEGIN some functions

fn get_gate_from_constant_bit(bit: u64) -> QubitRef {
    assert!((bit == 0) | (bit == 1));
    if bit == 1 {
        QubitRef::from(Qubit::ConstTrue)
    } else {
        QubitRef::from(Qubit::ConstFalse)
    }
}

fn get_qubit_from_bool(bit: bool) -> QubitRef {
    if bit {
        QubitRef::from(Qubit::ConstTrue)
    } else {
        QubitRef::from(Qubit::ConstFalse)
    }
}

fn is_constant(qubit_type: &QubitRef) -> bool {
    matches!(&*qubit_type.borrow(), Qubit::ConstFalse | Qubit::ConstTrue)
}

fn get_replacement_from_constant(sort: &NodeType, value_: u64) -> Vec<QubitRef> {
    let total_bits = sort.bitsize();
    let mut replacement: Vec<QubitRef> = Vec::new();
    let mut value = value_;
    for _ in 0..total_bits {
        replacement.push(get_gate_from_constant_bit(value % 2));
        value /= 2;
    }
    replacement
}

fn get_constant(gate_type: &QubitRef) -> Option<bool> {
    match &*gate_type.borrow() {
        Qubit::ConstFalse => Some(false),
        Qubit::ConstTrue => Some(true),
        _ => None,
    }
}

fn are_both_constants(const1: Option<bool>, const2: Option<bool>) -> bool {
    if let Some(_a) = const1 {
        if let Some(_b) = const2 {
            return true;
        }
    }
    false
}

fn are_there_false_constants(consts: Vec<Option<bool>>) -> bool {
    for c in consts.into_iter().flatten() {
        if !c {
            return true;
        }
    }
    false
}

fn are_there_true_constants(consts: Vec<Option<bool>>) -> bool {
    for c in consts.into_iter().flatten() {
        if c {
            return true;
        }
    }
    false
}

fn prepare_controls_for_mcx(
    controls: Vec<QubitRef>,
    target: QubitRef,
) -> (Option<bool>, Vec<QubitRef>) {
    let mut is_used: HashSet<HashableQubitRef> = HashSet::new();
    let target_key = HashableQubitRef::from(target);
    is_used.insert(target_key);

    let mut result: Vec<QubitRef> = Vec::new();
    for q in controls {
        if !is_constant(&q) {
            let key = HashableQubitRef::from(q.clone());
            if !is_used.contains(&key) {
                is_used.insert(key);
                result.push(q);
            }
        } else {
            let val = get_constant(&q);
            if are_there_false_constants(vec![val]) {
                return (Some(false), vec![]);
            }
        }
    }

    if result.is_empty() {
        (Some(true), result)
    } else {
        (None, result)
    }
}

// END some functions

// Begin implementation

pub struct QuantumCircuit<'a> {
    pub bad_state_qubits: Vec<QubitRef>,
    pub bad_state_nodes: Vec<NodeRef>,
    pub all_qubits: HashSet<QubitRef>,
    pub constraints: HashMap<HashableQubitRef, bool>, // this is for remainder and division, these are constraint based.
    pub input_qubits: Vec<(NodeRef, Vec<QubitRef>)>,
    pub mapping: HashMap<HashableNodeRef, HashMap<usize, Vec<QubitRef>>>, // maps a btor2 operator to its resulting bitvector of gates
    pub circuit_stack: Vec<UnitaryRef>,
    pub count_multiqubit_gates: u64,
    pub current_n: usize,
    pub current_state_nodes: HashMap<HashableNodeRef, usize>,
    pub dynamic_memory: Vec<QubitRef>,
    pub dm_index: usize,
    word_size: usize,
    model: &'a Model, // BTOR2 model
    use_dynamic_memory: bool,
}

impl<'a> QuantumCircuit<'a> {
    pub fn new(model_: &'a Model, word_size_: usize, use_dynamic_memory_: bool) -> Self {
        Self {
            bad_state_qubits: Vec::new(),
            bad_state_nodes: Vec::new(),
            constraints: HashMap::new(),
            all_qubits: HashSet::new(),
            input_qubits: Vec::new(),
            mapping: HashMap::new(),
            dynamic_memory: Vec::new(),
            dm_index: 0,
            circuit_stack: Vec::new(),
            current_state_nodes: HashMap::new(),
            model: model_,
            word_size: word_size_,
            count_multiqubit_gates: 0,
            current_n: 0,
            use_dynamic_memory: use_dynamic_memory_,
        }
    }

    fn not_gate(&mut self, a_qubit: &QubitRef) -> QubitRef {
        let a = get_constant(a_qubit);

        if let Some(a_const) = a {
            if a_const {
                QubitRef::from(Qubit::ConstFalse)
            } else {
                QubitRef::from(Qubit::ConstTrue)
            }
        } else {
            self.circuit_stack.push(UnitaryRef::from(Unitary::Not {
                input: a_qubit.clone(),
            }));
            a_qubit.clone()
        }
    }

    fn get_last_qubitset(&mut self, node: &NodeRef) -> Option<Vec<QubitRef>> {
        let key = HashableNodeRef::from(node.clone());

        if !self.mapping.contains_key(&key) {
            None
        } else {
            match *node.borrow() {
                Node::Const { .. } => {
                    if self.mapping.get(&key).unwrap().contains_key(&0) {
                        Some(self.mapping.get(&key).cloned().unwrap()[&0].clone())
                    } else {
                        None
                    }
                }
                Node::State { .. } => {
                    let replacements = self.mapping.get(&key).unwrap();
                    if let Some(replacement) = replacements.get(&(self.current_n - 1)) {
                        Some(replacement.clone())
                    } else {
                        let node_hash = HashableNodeRef::from(node.clone());
                        let index = self.current_state_nodes[&node_hash];
                        Some(replacements[&index].clone())
                    }
                }
                Node::Next { .. } | Node::Bad { .. } => None,
                _ => {
                    // Node::Input is included here?
                    let replacements = self.mapping.get(&key).unwrap();
                    if replacements.contains_key(&self.current_n) {
                        Some(replacements[&self.current_n].clone())
                    } else {
                        None
                    }
                }
            }
        }
    }

    fn uncompute(&mut self, current_replacement: Vec<QubitRef>) -> Vec<QubitRef> {
        let mut new_replacement: Vec<QubitRef> = vec![];

        // PERFORM SWAPS to start committing to memory
        self.circuit_stack
            .push(UnitaryRef::from(Unitary::Barrier {}));
        for qubit in current_replacement {
            match *qubit.borrow() {
                Qubit::ConstTrue | Qubit::ConstFalse => {
                    new_replacement.push(qubit.clone());
                }
                Qubit::QBit { .. } => self.circuit_stack.push(UnitaryRef::from(Unitary::Cnot {
                    control: qubit.clone(),
                    target: QubitRef::from(Qubit::QBit {
                        name: "random_name".to_string(),
                    }),
                })),
            }
        }
        self.circuit_stack
            .push(UnitaryRef::from(Unitary::Barrier {}));

        // move to the part that we want to uncompute. I.e before the last 2 swaps
        let mut i: i64 = self.circuit_stack.len() as i64;
        let mut count_barriers = 0;

        while count_barriers < 2 {
            count_barriers += match *self.circuit_stack[i as usize].borrow() {
                Unitary::Barrier => 1,
                _ => 0,
            };
            i -= 1;
        }
        if i > -1 {
            if let Unitary::Barrier = *self.circuit_stack[i as usize].borrow() {
                panic!("barrier should not be here!")
            }
        }

        // begin uncomputing
        let mut is_end_loop = false;
        while !is_end_loop {
            let gate_ = match *self.circuit_stack[i as usize].borrow() {
                Unitary::Barrier => None,
                _ => Some(self.circuit_stack[i as usize].clone()),
            };

            if let Some(gate) = gate_ {
                self.circuit_stack.push(gate);
            } else {
                is_end_loop = true;
            }
            i -= 1;
        }

        new_replacement
    }

    fn record_mapping(
        &mut self,
        node: &NodeRef,
        index: usize,
        replacement: Vec<QubitRef>,
    ) -> Vec<QubitRef> {
        let key = HashableNodeRef::from(node.clone());

        if !self.mapping.contains_key(&key) {
            self.mapping.insert(key.clone(), HashMap::new());
        }

        let replacements = self.mapping.get_mut(&key).unwrap();
        assert!(!replacements.contains_key(&index));
        replacements.insert(index, replacement.clone());

        self.current_state_nodes.insert(key, index);

        replacement
    }

    fn process_input(&mut self, sort: usize, node: &NodeRef, name: String) -> Vec<QubitRef> {
        if let Some(replacement) = self.get_last_qubitset(node) {
            replacement
        } else {
            let mut replacement: Vec<QubitRef> = Vec::new();
            for i in 0..sort {
                let name = format!("{}[bit={}]", name, i);
                replacement.push(QubitRef::from(Qubit::QBit { name }));
            }
            self.input_qubits.push((node.clone(), replacement.clone()));
            assert!(replacement.len() == sort);
            self.record_mapping(node, self.current_n, replacement)
        }
    }

    fn get_memory(&mut self, n: usize) -> Vec<QubitRef> {
        assert!(n > 0);
        let mut available_space = self.dynamic_memory.len() - self.dm_index;

        while available_space < n {
            self.dynamic_memory.push(QubitRef::from(Qubit::QBit {
                name: "dm".to_string(),
            }));
            available_space += 1;
        }

        let mut replacement: Vec<QubitRef> = Vec::new();

        while replacement.len() < n {
            replacement.push(self.dynamic_memory[self.dm_index].clone());
            self.dm_index += 1;
        }

        replacement
    }

    fn add_one_qubitset(
        &mut self,
        qubitset: &[QubitRef],
        target_set: Vec<QubitRef>,
    ) -> Vec<QubitRef> {
        let mut result: Vec<QubitRef> = vec![];

        let sort = qubitset.len();

        for index in 0..sort {
            for i in index..sort {
                let mut tmp_controls = target_set[index..((sort - 1 - i) + index)].to_vec();
                tmp_controls.push(qubitset[index].clone());
                let mut target = target_set[sort - 1 - i + index].clone();
                let (mcx_res, controls) = prepare_controls_for_mcx(tmp_controls, target.clone());

                if let Some(mcx_val) = mcx_res {
                    if mcx_val {
                        result.push(QubitRef::from(Qubit::ConstTrue));
                    } else {
                        result.push(QubitRef::from(Qubit::ConstFalse));
                    }
                } else {
                    if is_constant(&target) {
                        target = self.get_memory(1)[0].clone();
                    }
                    result.push(target.clone());
                    self.circuit_stack
                        .push(UnitaryRef::from(Unitary::Mcx { controls, target }))
                }
            }
        }

        assert!(result.len() == sort);
        result
    }

    fn add(&mut self, left_operand: &[QubitRef], right_operand: &[QubitRef]) -> Vec<QubitRef> {
        let mut replacement: Vec<QubitRef> = vec![];

        for _ in 0..left_operand.len() {
            replacement.push(QubitRef::from(Qubit::ConstFalse));
        }

        replacement = self.add_one_qubitset(left_operand, replacement);
        replacement = self.add_one_qubitset(right_operand, replacement);

        replacement
    }

    fn sub(&mut self, left_operand: &[QubitRef], right_operand: &[QubitRef]) -> Vec<QubitRef> {
        let (to_uncompute, negated_right) = self.twos_complement(right_operand);

        let replacement = self.add(left_operand, &negated_right);

        // uncompute twos complement
        for gate in to_uncompute.iter().rev() {
            self.circuit_stack.push(gate.clone());
        }
        replacement
    }

    fn twos_complement(&mut self, qubitset: &[QubitRef]) -> (Vec<UnitaryRef>, Vec<QubitRef>) {
        let mut gates_to_uncompute: Vec<UnitaryRef> = Vec::new();
        let mut result1: Vec<QubitRef> = Vec::new();

        for qubit in qubitset {
            if let Some(val) = get_constant(qubit) {
                if val {
                    result1.push(QubitRef::from(Qubit::ConstTrue));
                } else {
                    result1.push(QubitRef::from(Qubit::ConstFalse));
                }
            } else {
                result1.push(qubit.clone());
                gates_to_uncompute.push(UnitaryRef::from(Unitary::Not {
                    input: qubit.clone(),
                }));
                self.circuit_stack.push(UnitaryRef::from(Unitary::Not {
                    input: qubit.clone(),
                }));
            }
        }

        let sort = qubitset.len();
        let mut result2: Vec<QubitRef> = Vec::new();
        for i in 0..sort - 1 {
            let target = result1[sort - i - 1].clone();
            let tmp_controls = result1[..(sort - i - 1)].to_vec();
            let (mcx_res, controls) = prepare_controls_for_mcx(tmp_controls, target.clone());

            if let Some(mcx_val) = mcx_res {
                result2.push(get_qubit_from_bool(mcx_val));
            } else {
                result2.push(target.clone());
                gates_to_uncompute.push(UnitaryRef::from(Unitary::Mcx {
                    controls: controls.clone(),
                    target: target.clone(),
                }));
                self.circuit_stack
                    .push(UnitaryRef::from(Unitary::Mcx { controls, target }));
            }
        }
        result2.push(result1[sort - 1].clone());

        assert!(result2.len() == result1.len());
        assert!(result2.len() == qubitset.len());

        if let Some(val) = get_constant(&result2[0]) {
            result2[0] = get_qubit_from_bool(!val);
        } else {
            gates_to_uncompute.push(UnitaryRef::from(Unitary::Not {
                input: result2[0].clone(),
            }));
            self.circuit_stack.push(UnitaryRef::from(Unitary::Not {
                input: result2[0].clone(),
            }));
        }

        (gates_to_uncompute, result2)
    }

    fn multiply_word_by_bit(
        &mut self,
        word: Vec<QubitRef>,
        bit: QubitRef,
        shift: usize,
    ) -> (usize, Vec<UnitaryRef>, Vec<QubitRef>) {
        let mut gates_to_uncompute: Vec<UnitaryRef> = Vec::new();
        let mut result: Vec<QubitRef> = Vec::new();
        let mut used_memory = 0;

        if let Some(val) = get_constant(&bit) {
            if val {
                (used_memory, gates_to_uncompute, word)
            } else {
                while result.len() < word.len() {
                    result.push(QubitRef::from(Qubit::ConstFalse));
                }
                (used_memory, gates_to_uncompute, result)
            }
        } else {
            let mut i = 0;
            let mut s = 0;

            while result.len() < word.len() {
                if s < shift {
                    result.push(QubitRef::from(Qubit::ConstFalse));
                    s += 1;
                } else {
                    if let Some(val) = get_constant(&word[i]) {
                        if val {
                            result.push(bit.clone());
                        } else {
                            result.push(QubitRef::from(Qubit::ConstFalse));
                        }
                    } else {
                        used_memory += 1;
                        let target = self.get_memory(1)[0].clone();
                        gates_to_uncompute.push(UnitaryRef::from(Unitary::Mcx {
                            controls: vec![word[i].clone(), bit.clone()],
                            target: target.clone(),
                        }));
                        self.circuit_stack.push(UnitaryRef::from(Unitary::Mcx {
                            controls: vec![word[i].clone(), bit.clone()],
                            target,
                        }));
                    }
                    i += 1;
                }
            }
            assert!(result.len() == word.len());
            (used_memory, gates_to_uncompute, result)
        }
    }

    fn mul(&mut self, left_operand: Vec<QubitRef>, right_operand: Vec<QubitRef>) -> Vec<QubitRef> {
        let mut replacement: Vec<QubitRef> = Vec::new();

        for _ in 0..left_operand.len() {
            replacement.push(QubitRef::from(Qubit::ConstFalse));
        }

        for (index, bit) in left_operand.iter().enumerate() {
            let (used_memory, gates_to_uncompute, result) =
                self.multiply_word_by_bit(right_operand.clone(), bit.clone(), index);

            replacement = self.add_one_qubitset(&result, replacement);

            // uncompute ancillas
            for gate in gates_to_uncompute.iter().rev() {
                self.circuit_stack.push(gate.clone());
            }

            self.dm_index -= used_memory;
        }

        replacement
    }

    fn eq(&mut self, left_operand: &[QubitRef], right_operand: &[QubitRef]) -> Vec<QubitRef> {
        let mut controls: Vec<QubitRef> = vec![];

        for (l_qubit, r_qubit) in left_operand.iter().zip(right_operand.iter()) {
            let const_l = get_constant(l_qubit);
            let const_r = get_constant(r_qubit);

            if are_both_constants(const_l, const_r) {
                if const_l.unwrap() != const_r.unwrap() {
                    return vec![QubitRef::from(Qubit::ConstFalse)];
                }
            } else if are_there_true_constants(vec![const_l, const_r]) {
                if is_constant(l_qubit) {
                    controls.push(r_qubit.clone());
                } else {
                    controls.push(l_qubit.clone());
                }
            } else if are_there_false_constants(vec![const_l, const_r]) {
                let control: QubitRef;
                if is_constant(l_qubit) {
                    control = r_qubit.clone();
                } else {
                    control = l_qubit.clone();
                }

                let target: QubitRef;
                if self.use_dynamic_memory {
                    target = self.get_memory(1)[0].clone();
                } else {
                    target = QubitRef::from(Qubit::QBit {
                        name: "eq_ancilla".to_string(),
                    });
                }
                controls.push(target.clone());
                self.circuit_stack.push(UnitaryRef::from(Unitary::Not {
                    input: control.clone(),
                }));
                self.circuit_stack.push(UnitaryRef::from(Unitary::Cnot {
                    control: control.clone(),
                    target,
                }));
                self.circuit_stack
                    .push(UnitaryRef::from(Unitary::Not { input: control }));
            } else {
                // no constants
                let target: QubitRef;
                if self.use_dynamic_memory {
                    target = self.get_memory(1)[0].clone();
                } else {
                    target = QubitRef::from(Qubit::QBit {
                        name: "eq_ancilla".to_string(),
                    });
                }
                controls.push(target.clone());
                self.circuit_stack.push(UnitaryRef::from(Unitary::Cnot {
                    control: l_qubit.clone(),
                    target: target.clone(),
                }));
                self.circuit_stack.push(UnitaryRef::from(Unitary::Cnot {
                    control: r_qubit.clone(),
                    target: target.clone(),
                }));
                self.circuit_stack
                    .push(UnitaryRef::from(Unitary::Not { input: target }));
            }
        }
        let replacement: Vec<QubitRef>;
        if controls.is_empty() {
            replacement = vec![QubitRef::from(Qubit::ConstTrue)];
        } else {
            let target: QubitRef;
            if self.use_dynamic_memory {
                target = self.get_memory(1)[0].clone();
            } else {
                target = QubitRef::from(Qubit::QBit {
                    name: "eq_ancilla".to_string(),
                });
            }

            replacement = vec![target.clone()];
            self.circuit_stack
                .push(UnitaryRef::from(Unitary::Mcx { controls, target }));
        }
        // TODO: there is uncomputing that can be done here
        replacement
    }

    fn insert_into_contrants(&mut self, qubit: &QubitRef, value: bool) {
        let key = HashableQubitRef::from(qubit.clone());
        assert!(!self.constraints.contains_key(&key));
        self.constraints.insert(key, value);
    }

    fn div(&mut self, left: &NodeRef, right: &NodeRef) -> (Vec<QubitRef>, Vec<QubitRef>) {
        let mut left_operand = self.process(left);
        let mut right_operand = self.process(right);
        let mut c: Vec<QubitRef> = Vec::new();
        let mut r: Vec<QubitRef> = Vec::new();

        left_operand.push(QubitRef::from(Qubit::ConstFalse));
        right_operand.push(QubitRef::from(Qubit::ConstFalse));

        let sort = left_operand.len();

        while c.len() < sort {
            c.push(QubitRef::from(Qubit::QBit {
                name: "div_c".to_string(),
            }));
            r.push(QubitRef::from(Qubit::QBit {
                name: "div_r".to_string(),
            }));
        }

        let res_mul = self.mul(c.clone(), right_operand);
        let res_sum = self.add(&res_mul, &r);

        let res_eq = self.eq(&res_sum, &left_operand);

        assert!(res_eq.len() == 1);

        self.insert_into_contrants(&res_eq[0], true);
        self.insert_into_contrants(&res_mul[sort - 1], false);
        self.insert_into_contrants(&res_sum[sort - 1], false);

        (c, r)
    }

    fn process(&mut self, node: &NodeRef) -> Vec<QubitRef> {
        if let Some(replacement) = self.get_last_qubitset(node) {
            return replacement;
        }
        match &*node.borrow() {
            Node::Const { sort, imm, .. } => {
                assert!(self.current_n == 0);
                let replacement = get_replacement_from_constant(sort, *imm);
                assert!(replacement.len() == sort.bitsize());
                self.record_mapping(node, self.current_n, replacement)
            }
            Node::State {
                init: None,
                sort,
                name,
                ..
            } => {
                // this is an input
                let name = name.as_deref().unwrap_or("?");
                self.process_input(sort.bitsize(), node, name.to_string())
            }
            Node::Input { sort, name, .. } => {
                self.process_input(sort.bitsize(), node, name.to_string())
            }
            Node::State { sort, init, .. } => {
                // This is a normal state
                assert!(self.current_n == 0);
                let mut replacement = Vec::new();
                if let Some(value) = init {
                    replacement = self.process(value);
                } else {
                    for _ in 0..sort.bitsize() {
                        replacement.push(QubitRef::from(Qubit::ConstFalse));
                    }
                }
                assert!(replacement.len() == sort.bitsize());
                self.record_mapping(node, self.current_n, replacement)
            }
            Node::Not { value, .. } => {
                let bitvector = self.process(value);
                let mut replacement: Vec<QubitRef> = Vec::new();
                for bit in &bitvector {
                    replacement.push(self.not_gate(bit));
                }
                assert!(replacement.len() == bitvector.len());
                self.record_mapping(node, self.current_n, replacement)
            }
            Node::Bad { cond, .. } => {
                self.circuit_stack.push(UnitaryRef::from(Unitary::Barrier));
                let replacement = self.process(cond);
                assert!(replacement.len() == 1);
                if self.use_dynamic_memory {
                    let new_replacement = self.uncompute(replacement);
                    self.dm_index = 0;
                    self.record_mapping(node, self.current_n, new_replacement)
                } else {
                    self.record_mapping(node, self.current_n, replacement)
                }
            }
            Node::And { left, right, .. } => {
                let left_operand = self.process(left);
                let right_operand = self.process(right);
                let mut replacement: Vec<QubitRef> = vec![];

                for (l_qubit, r_qubit) in left_operand.iter().zip(right_operand.iter()) {
                    let const_l = get_constant(l_qubit);
                    let const_r = get_constant(r_qubit);
                    if are_both_constants(const_l, const_r) {
                        let val = const_l.unwrap() && const_r.unwrap();
                        replacement.push(get_qubit_from_bool(val));
                    } else if are_there_false_constants(vec![const_r, const_l]) {
                        replacement.push(QubitRef::from(Qubit::ConstFalse));
                    } else if are_there_true_constants(vec![const_l, const_r]) {
                        let control: QubitRef;
                        if is_constant(l_qubit) {
                            // l_qubit must be true
                            control = r_qubit.clone();
                        } else {
                            // const_r must be true
                            control = l_qubit.clone();
                        }
                        let target: QubitRef;

                        if self.use_dynamic_memory {
                            target = self.get_memory(1)[0].clone();
                        } else {
                            target = QubitRef::from(Qubit::QBit {
                                name: "and".to_string(),
                            });
                        }

                        self.circuit_stack
                            .push(UnitaryRef::from(Unitary::Cnot { control, target }))
                    } else {
                        // there are no constants
                        let target: QubitRef;
                        if self.use_dynamic_memory {
                            target = self.get_memory(1)[0].clone();
                        } else {
                            target = QubitRef::from(Qubit::QBit {
                                name: "and".to_string(),
                            });
                        }

                        self.circuit_stack.push(UnitaryRef::from(Unitary::Mcx {
                            controls: vec![l_qubit.clone(), r_qubit.clone()],
                            target,
                        }))
                    }
                }
                self.record_mapping(node, self.current_n, replacement)
            }
            Node::Ext { from, value, .. } => {
                let mut replacement: Vec<QubitRef> = self.process(value);
                assert!(replacement.len() == from.bitsize());
                for _ in 0..(self.word_size - from.bitsize()) {
                    replacement.push(QubitRef::from(Qubit::ConstFalse));
                }
                self.record_mapping(node, self.current_n, replacement)
            }
            Node::Eq { left, right, .. } => {
                let left_operand = self.process(left);
                let right_operand = self.process(right);

                let replacement = self.eq(&left_operand, &right_operand);
                self.record_mapping(node, self.current_n, replacement)
            }
            Node::Add { left, right, .. } => {
                let left_operand = self.process(left);
                let right_operand = self.process(right);

                let replacement = self.add(&left_operand, &right_operand);

                self.record_mapping(node, self.current_n, replacement)
            }
            Node::Ite {
                cond, left, right, ..
            } => {
                let cond_operand = self.process(cond);

                assert!(cond_operand.len() == 1);
                let mut replacement: Vec<QubitRef>;
                if let Some(cond_val) = get_constant(&cond_operand[0]) {
                    if cond_val {
                        replacement = self.process(left);
                    } else {
                        replacement = self.process(right);
                    }
                } else {
                    replacement = Vec::new();
                    let left_operand = self.process(left);
                    let right_operand = self.process(right);

                    for (l_qubit, r_qubit) in left_operand.iter().zip(right_operand.iter()) {
                        let const_l = get_constant(l_qubit);
                        let const_r = get_constant(r_qubit);

                        if are_both_constants(const_l, const_r) {
                            if const_l.unwrap() == const_r.unwrap() {
                                replacement.push(l_qubit.clone());
                            } else if const_l.unwrap() {
                                replacement.push(cond_operand[0].clone());
                            } else {
                                let target: QubitRef;

                                if self.use_dynamic_memory {
                                    target = self.get_memory(1)[0].clone();
                                } else {
                                    target = QubitRef::from(Qubit::QBit {
                                        name: "ancilla_ite".to_string(),
                                    });
                                }
                                // TODO: Maybe is not necesary to request memory here for target?
                                replacement.push(target.clone());
                                self.circuit_stack.push(UnitaryRef::from(Unitary::Cnot {
                                    control: cond_operand[0].clone(),
                                    target,
                                }));
                                self.circuit_stack.push(UnitaryRef::from(Unitary::Not {
                                    input: cond_operand[0].clone(),
                                }));
                            }
                        } else if are_there_false_constants(vec![const_l, const_r]) {
                            let target: QubitRef;

                            if self.use_dynamic_memory {
                                target = self.get_memory(1)[0].clone();
                            } else {
                                target = QubitRef::from(Qubit::QBit {
                                    name: "ancilla_ite".to_string(),
                                });
                            }

                            if is_constant(l_qubit) {
                                self.circuit_stack.push(UnitaryRef::from(Unitary::Not {
                                    input: cond_operand[0].clone(),
                                }));
                                self.circuit_stack.push(UnitaryRef::from(Unitary::Mcx {
                                    controls: vec![cond_operand[0].clone(), r_qubit.clone()],
                                    target,
                                }));
                                self.circuit_stack.push(UnitaryRef::from(Unitary::Not {
                                    input: cond_operand[0].clone(),
                                }));
                            } else {
                                self.circuit_stack.push(UnitaryRef::from(Unitary::Mcx {
                                    controls: vec![cond_operand[0].clone(), l_qubit.clone()],
                                    target,
                                }));
                            }
                        } else {
                            // TODO: Optimize when there is only one constant and its true?

                            // no constants
                            let target: QubitRef;
                            if self.use_dynamic_memory {
                                target = self.get_memory(1)[0].clone();
                            } else {
                                target = QubitRef::from(Qubit::QBit {
                                    name: "ancilla_ite".to_string(),
                                });
                            }

                            self.circuit_stack.push(UnitaryRef::from(Unitary::Mcx {
                                controls: vec![cond_operand[0].clone(), l_qubit.clone()],
                                target: target.clone(),
                            }));

                            self.circuit_stack.push(UnitaryRef::from(Unitary::Not {
                                input: cond_operand[0].clone(),
                            }));
                            self.circuit_stack.push(UnitaryRef::from(Unitary::Mcx {
                                controls: vec![cond_operand[0].clone(), r_qubit.clone()],
                                target,
                            }));
                            self.circuit_stack.push(UnitaryRef::from(Unitary::Not {
                                input: cond_operand[0].clone(),
                            }));
                        }
                    }
                }

                self.record_mapping(node, self.current_n, replacement)
            }
            Node::Sub { left, right, .. } => {
                let left_operand = self.process(left);
                let right_operand = self.process(right);

                let replacement = self.sub(&left_operand, &right_operand);

                self.record_mapping(node, self.current_n, replacement)
            }
            Node::Ult { left, right, .. } => {
                let mut left_operand = self.process(left);
                let mut right_operand = self.process(right);

                left_operand.push(QubitRef::from(Qubit::ConstFalse));
                right_operand.push(QubitRef::from(Qubit::ConstFalse));

                let result = self.sub(&left_operand, &right_operand);

                self.record_mapping(node, self.current_n, vec![result.last().unwrap().clone()])
            }
            Node::Mul { left, right, .. } => {
                let left_operand = self.process(left);
                let right_operand = self.process(right);

                let replacement = self.mul(left_operand, right_operand);

                self.record_mapping(node, self.current_n, replacement)
            }
            Node::Div { left, right, .. } => {
                let (replacement, _) = self.div(left, right);
                self.record_mapping(node, self.current_n, replacement)
            }
            Node::Rem { left, right, .. } => {
                let (_, replacement) = self.div(left, right);
                self.record_mapping(node, self.current_n, replacement)
            }
            Node::Next { state, next, .. } => {
                let _ = self.process(state);
                self.circuit_stack.push(UnitaryRef::from(Unitary::Barrier));
                let replacement = self.process(next);
                if self.use_dynamic_memory {
                    let new_replacement = self.uncompute(replacement);
                    self.dm_index = 0;
                    self.record_mapping(node, self.current_n, new_replacement)
                } else {
                    self.record_mapping(state, self.current_n, replacement)
                }
            }
            Node::Read { .. } | Node::Write { .. } => {
                unimplemented!()
            }
            _ => {
                panic!("Unknown BTOR2 node!");
            }
        }
    }

    pub fn process_model<W>(mut self, out: W, unroll_depth: usize) -> Result<()>
    where
        W: Write,
    {
        assert!(self.bad_state_qubits.is_empty());
        assert!(self.bad_state_nodes.is_empty());
        assert!(self.input_qubits.is_empty());
        assert!(self.circuit_stack.is_empty());
        assert!(self.word_size == 64 || self.word_size == 32);

        for i in 0..unroll_depth {
            self.current_n = i;
            for sequential in &self.model.sequentials {
                if let Node::Next { .. } = &*sequential.borrow() {
                    let _ = self.process(sequential);
                    // TODO: flush memory, and reset ancillas
                } else {
                    panic!("expecting 'Next' node here");
                }
            }

            for bad_state in &self.model.bad_states_sequential {
                let bitblasted_bad_state = self.process(bad_state);
                assert!(bitblasted_bad_state.len() == 1);
                self.bad_state_qubits.push(bitblasted_bad_state[0].clone());
            }
        }
        self.write_model(out)
    }

    pub fn write_model<W>(self, mut _out: W) -> Result<()>
    where
        W: Write,
    {
        unimplemented!()
    }
}
