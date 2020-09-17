use crate::control_flow_graph::{CFGNode, ControlFlowGraph};
use crate::dominator_tree::PostDominatorTree;
use llvm_ir::Name;
use log::debug;
use petgraph::prelude::{DfsPostOrder, DiGraphMap, Direction};
use petgraph::visit::Walker;
use std::collections::HashSet;

pub struct ControlDependenceGraph<'m> {
    /// The graph itself. An edge from bbX to bbY indicates that bbX has an
    /// immediate control dependence on bbY. A path from bbX to bbY indicates
    /// that bbX has a control dependence on bbY.
    graph: DiGraphMap<CFGNode<'m>, ()>,

    /// Entry node for the function
    pub(crate) entry_node: CFGNode<'m>,
}

impl<'m> ControlDependenceGraph<'m> {
    pub(crate) fn new(cfg: &ControlFlowGraph<'m>, postdomtree: &PostDominatorTree<'m>) -> Self {
        // algorithm thanks to Cytron, Ferrante, Rosen, et al. "Efficiently Computing Static Single Assignment Form and the Control Dependence Graph"
        // https://www.cs.utexas.edu/~pingali/CS380C/2010/papers/ssaCytron.pdf (Figure 10)

        let mut graph = DiGraphMap::new();

        for block_x in DfsPostOrder::new(&postdomtree.graph, CFGNode::Return).iter(&postdomtree.graph) {
            let mut postdominance_frontier_of_x = vec![];
            debug!("--- Computing postdominance frontier of {} ---", block_x);
            for block_y in cfg.preds_as_nodes(block_x) {
                debug!("Processing predecessor {}", block_y);
                if postdomtree.ipostdom_of_cfgnode(block_y) != Some(block_x) {
                    debug!("  pushing it");
                    postdominance_frontier_of_x.push(block_y);
                }
            }
            for block_z in postdomtree.children_of_cfgnode(block_x) {
                debug!("block_z is {}, which is immediately postdominated by {}", block_z, block_x);
                // we should have already computed all of the outgoing edges from block_z
                for block_y in graph.neighbors_directed(block_z, Direction::Outgoing) {
                    debug!("Processing node {}, in the postdominance frontier of {}", block_y, block_z);
                    if postdomtree.ipostdom_of_cfgnode(block_y) != Some(block_x) {
                        debug!("  pushing it");
                        postdominance_frontier_of_x.push(block_y);
                    }
                }
            }
            for node in postdominance_frontier_of_x {
                graph.add_edge(block_x, node, ());
            }
        }

        Self {
            graph,
            entry_node: cfg.entry_node,
        }
    }

    /// Get the blocks that `block` has an immediate control dependency on.
    pub fn get_imm_control_dependencies<'s>(&'s self, block: &'m Name) -> impl Iterator<Item = &'m Name> + 's {
        self.get_imm_control_dependencies_of_cfgnode(CFGNode::Block(block))
    }

    pub(crate) fn get_imm_control_dependencies_of_cfgnode<'s>(&'s self, node: CFGNode<'m>) -> impl Iterator<Item = &'m Name> + 's {
        self.graph.neighbors_directed(node, Direction::Outgoing).map(|node| match node {
            CFGNode::Block(block) => block,
            CFGNode::Return => panic!("Nothing should be control-dependent on Return"),
        })
    }

    /// Get the blocks that `block` has a control dependency on (including
    /// transitively).
    ///
    /// This is the block's immediate control dependencies, along with all the
    /// control dependencies of those dependencies, and so on recursively.
    pub fn get_control_dependencies<'s>(&'s self, block: &'m Name) -> impl Iterator<Item = &'m Name> + 's {
        ControlDependenciesIterator::new(self, block)
    }

    /// Get the blocks that have an immediate control dependency on `block`
    pub fn get_imm_control_dependents<'s>(&'s self, block: &'m Name) -> impl Iterator<Item = CFGNode<'m>> + 's {
        self.get_imm_control_dependents_of_cfgnode(CFGNode::Block(block))
    }

    pub(crate) fn get_imm_control_dependents_of_cfgnode<'s>(&'s self, node: CFGNode<'m>) -> impl Iterator<Item = CFGNode<'m>> + 's {
        self.graph.neighbors_directed(node, Direction::Incoming)
    }

    /// Get the blocks that have a control dependency on `block` (including
    /// transitively).
    ///
    /// This is the block's immediate control dependents, along with all the
    /// control dependents of those dependents, and so on recursively.
    pub fn get_control_dependents<'s>(&'s self, block: &'m Name) -> impl Iterator<Item = CFGNode<'m>> + 's {
        ControlDependentsIterator::new(self, block)
    }

    /// Does `block_a` have a control dependency on `block_b`?
    pub fn is_control_dependent(&self, block_a: &'m Name, block_b: &'m Name) -> bool {
        petgraph::algo::has_path_connecting(&self.graph, CFGNode::Block(block_a), CFGNode::Block(block_b), None)
    }

    /// Get the `Name` of the entry block for the function
    pub fn entry(&self) -> &'m Name {
        match self.entry_node {
            CFGNode::Block(block) => block,
            CFGNode::Return => panic!("Return node should not be entry"), // perhaps you tried to call this on a reversed CFG? In-crate users can use the `entry_node` field directly if they need to account for the possibility of a reversed CFG
        }
    }
}

struct ControlDependenciesIterator<'m> {
    /// Currently implemented by computing all dependencies into a `HashSet` at
    /// the beginning and then iterating over that `HashSet`. But this may
    /// change, hence the opaque interface
    deps: std::collections::hash_set::IntoIter<&'m Name>,
}

impl<'m> ControlDependenciesIterator<'m> {
    /// Get a new iterator which will iterate over the control dependencies of `block`
    fn new(cdg: &ControlDependenceGraph<'m>, block: &'m Name) -> Self {
        let mut worklist: Vec<&'m Name> = cdg.get_imm_control_dependencies(block).collect();
        let mut deps: HashSet<&'m Name> = HashSet::new();
        while let Some(block) = worklist.pop() {
            deps.insert(block);
            worklist.extend(cdg.get_imm_control_dependencies(block))
        }
        Self {
            deps: deps.into_iter(),
        }
    }
}

impl<'m> Iterator for ControlDependenciesIterator<'m> {
    type Item = &'m Name;

    fn next(&mut self) -> Option<&'m Name> {
        self.deps.next()
    }
}

struct ControlDependentsIterator<'m> {
    /// Currently implemented by computing all dependents into a `HashSet` at the
    /// beginning and then iterating over that `HashSet`. But this may change,
    /// hence the opaque interface
    deps: std::collections::hash_set::IntoIter<CFGNode<'m>>,
}

impl<'m> ControlDependentsIterator<'m> {
    /// Get a new iterator which will iterate over the control dependents of `block`
    fn new(cdg: &ControlDependenceGraph<'m>, block: &'m Name) -> Self {
        let mut worklist: Vec<CFGNode<'m>> = cdg.get_imm_control_dependents(block).collect();
        let mut deps: HashSet<CFGNode<'m>> = HashSet::new();
        while let Some(node) = worklist.pop() {
            if deps.insert(node) {
                worklist.extend(cdg.get_imm_control_dependents_of_cfgnode(node))
            }
        }
        Self {
            deps: deps.into_iter(),
        }
    }
}

impl<'m> Iterator for ControlDependentsIterator<'m> {
    type Item = CFGNode<'m>;

    fn next(&mut self) -> Option<CFGNode<'m>> {
        self.deps.next()
    }
}
