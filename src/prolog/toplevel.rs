use prolog_parser::ast::*;
use prolog_parser::parser::*;
use prolog_parser::tabled_rc::*;

use prolog::ast::*;
use prolog::iterators::*;
use prolog::machine::*;
use prolog::num::*;

use std::collections::{HashSet, VecDeque};
use std::cell::{Cell, RefCell};
use std::io::Read;
use std::mem;
use std::rc::Rc;

fn setup_fact(term: Term) -> Result<Term, ParserError>
{
    match term {
        Term::Clause(..) | Term::Constant(_, Constant::Atom(_)) =>
            Ok(term),
        _ =>
            Err(ParserError::InadmissibleFact)
    }
}

fn setup_op_decl(mut terms: Vec<Box<Term>>) -> Result<OpDecl, ParserError>
{
    let name = match *terms.pop().unwrap() {
        Term::Constant(_, Constant::Atom(name)) => name,
        _ => return Err(ParserError::InconsistentEntry)
    };

    let spec = match *terms.pop().unwrap() {
        Term::Constant(_, Constant::Atom(name)) => name,
        _ => return Err(ParserError::InconsistentEntry)
    };

    let prec = match *terms.pop().unwrap() {
        Term::Constant(_, Constant::Number(Number::Integer(bi))) =>
            match bi.to_usize() {
                Some(n) if n <= 1200 => n,
                _ => return Err(ParserError::InconsistentEntry)
            },
        _ => return Err(ParserError::InconsistentEntry)
    };

    match spec.as_str() {
        "xfx" => Ok(OpDecl(prec, XFX, name)),
        "xfy" => Ok(OpDecl(prec, XFY, name)),
        "yfx" => Ok(OpDecl(prec, YFX, name)),
        "fx"  => Ok(OpDecl(prec, FX, name)),
        "fy"  => Ok(OpDecl(prec, FY, name)),
        "xf"  => Ok(OpDecl(prec, XF, name)),
        "yf"  => Ok(OpDecl(prec, YF, name)),
        _     => Err(ParserError::InconsistentEntry)
    }
}

fn setup_predicate_export(mut term: Term) -> Result<PredicateKey, ParserError>
{
    match term {
        Term::Clause(_, ref name, ref mut terms, Some(Fixity::In))
            if name.as_str() == "/" && terms.len() == 2 => {
                let arity = *terms.pop().unwrap();
                let name  = *terms.pop().unwrap();

                let arity = arity.to_constant().and_then(|c| c.to_integer())
                    .and_then(|n| if !n.is_negative() { n.to_usize() } else { None })
                    .ok_or(ParserError::InvalidModuleExport)?;

                let name = name.to_constant().and_then(|c| c.to_atom())
                    .ok_or(ParserError::InvalidModuleExport)?;

                Ok((name, arity))
            },
        _ => Err(ParserError::InvalidModuleExport)
    }
}

fn setup_module_decl(mut terms: Vec<Box<Term>>) -> Result<ModuleDecl, ParserError>
{
    let mut export_list = *terms.pop().unwrap();
    let name = terms.pop().unwrap().to_constant().and_then(|c| c.to_atom())
        .ok_or(ParserError::InvalidModuleDecl)?;

    let mut exports = Vec::new();

    while let Term::Cons(_, t1, t2) = export_list {
        exports.push(setup_predicate_export(*t1)?);
        export_list = *t2;
    }

    if export_list.to_constant() != Some(Constant::EmptyList) {
        Err(ParserError::InvalidModuleDecl)
    } else {
        Ok(ModuleDecl { name, exports })
    }
}

fn setup_use_module_decl(mut terms: Vec<Box<Term>>) -> Result<ClauseName, ParserError>
{
    match *terms.pop().unwrap() {
        Term::Clause(_, ref name, ref mut terms, None)
            if name.as_str() == "library" && terms.len() == 1 => {
                terms.pop().unwrap().to_constant()
                    .and_then(|c| c.to_atom())
                    .ok_or(ParserError::InvalidUseModuleDecl)
            },
        _ => Err(ParserError::InvalidUseModuleDecl)
    }
}

type UseModuleExport = (ClauseName, Vec<PredicateKey>);

fn setup_qualified_import(mut terms: Vec<Box<Term>>) -> Result<UseModuleExport, ParserError>
{
    let mut export_list = *terms.pop().unwrap();
    let name = match *terms.pop().unwrap() {
        Term::Clause(_, ref name, ref mut terms, None)
            if name.as_str() == "library" && terms.len() == 1 => {
                terms.pop().unwrap().to_constant()
                    .and_then(|c| c.to_atom())
                    .ok_or(ParserError::InvalidUseModuleDecl)
            },
        _ => Err(ParserError::InvalidUseModuleDecl)
    }?;

    let mut exports = Vec::new();

    while let Term::Cons(_, t1, t2) = export_list {
        exports.push(setup_predicate_export(*t1)?);
        export_list = *t2;
    }

    if export_list.to_constant() != Some(Constant::EmptyList) {
        Err(ParserError::InvalidModuleDecl)
    } else {
        Ok((name, exports))
    }
}

fn setup_declaration(term: Term) -> Result<Declaration, ParserError>
{
    match term {
        Term::Clause(_, name, mut terms, _) =>
            if name.as_str() == "op" && terms.len() == 3 {
                Ok(Declaration::Op(setup_op_decl(terms)?))
            } else if name.as_str() == "module" && terms.len() == 2 {
                Ok(Declaration::Module(setup_module_decl(terms)?))
            } else if name.as_str() == "use_module" && terms.len() == 1 {
                Ok(Declaration::UseModule(setup_use_module_decl(terms)?))
            } else if name.as_str() == "use_module" && terms.len() == 2 {
                let (name, exports) = setup_qualified_import(terms)?;
                Ok(Declaration::UseQualifiedModule(name, exports))
            } else if name.as_str() == "non_counted_backtracking" && terms.len() == 1 {
                let (name, arity) = setup_predicate_export(*terms.pop().unwrap())?;
                Ok(Declaration::NonCountedBacktracking(name, arity))
            } else {
                Err(ParserError::InconsistentEntry)
            },
        _ => return Err(ParserError::InconsistentEntry)
    }
}

fn is_consistent(tl: &TopLevel, clauses: &Vec<PredicateClause>) -> bool
{
    match clauses.first() {
        Some(ref cl) => tl.name() == cl.name() && tl.arity() == cl.arity(),
        None => true
    }
}

fn deque_to_packet(head: TopLevel, deque: VecDeque<TopLevel>) -> TopLevelPacket
{
    match head {
        TopLevel::Query(query) => TopLevelPacket::Query(query, Vec::from(deque)),
        tl => TopLevelPacket::Decl(tl, Vec::from(deque))
    }
}

fn merge_clauses(tls: &mut VecDeque<TopLevel>) -> Result<TopLevel, ParserError>
{
    let mut clauses: Vec<PredicateClause> = vec![];

    while let Some(tl) = tls.pop_front() {
        match tl {
            TopLevel::Query(_) if clauses.is_empty() && tls.is_empty() =>
                return Ok(tl),
            TopLevel::Declaration(_) if clauses.is_empty() =>
                return Ok(tl),
            TopLevel::Query(_) =>
                return Err(ParserError::InconsistentEntry),
            TopLevel::Fact(_) if is_consistent(&tl, &clauses) =>
                if let TopLevel::Fact(fact) = tl {
                    let clause = PredicateClause::Fact(fact);
                    clauses.push(clause);
                },
            TopLevel::Rule(_) if is_consistent(&tl, &clauses) =>
                if let TopLevel::Rule(rule) = tl {
                    let clause = PredicateClause::Rule(rule);
                    clauses.push(clause);
                },
            TopLevel::Predicate(_) if is_consistent(&tl, &clauses) =>
                if let TopLevel::Predicate(pred) = tl {
                    clauses.extend(pred.clauses().into_iter())
                },
            _ => {
                tls.push_front(tl);
                break;
            }
        }
    }

    if clauses.is_empty() {
        Err(ParserError::InconsistentEntry)
    } else {
        Ok(TopLevel::Predicate(Predicate(clauses)))
    }
}

fn append_preds(preds: &mut Vec<PredicateClause>) -> Predicate {
    Predicate(mem::replace(preds, vec![]))
}

fn unfold_by_str_once(term: &mut Term, s: &str) -> Option<(Term, Term)>
{
    if let &mut Term::Clause(_, ref name, ref mut subterms, _) = term {
        if name.as_str() == s && subterms.len() == 2 {
            let snd = *subterms.pop().unwrap();
            let fst = *subterms.pop().unwrap();

            return Some((fst, snd));
        }
    }

    None
}

fn unfold_by_str(mut term: Term, s: &str) -> Vec<Term>
{
    let mut terms = vec![];

    while let Some((fst, snd)) = unfold_by_str_once(&mut term, s) {
        terms.push(fst);
        term = snd;
    }

    terms.push(term);
    terms
}

fn fold_by_str(mut terms: Vec<Term>, mut term: Term, sym: ClauseName) -> Term
{
    while let Some(prec) = terms.pop() {
        term = Term::Clause(Cell::default(), sym.clone(),
                            vec![Box::new(prec), Box::new(term)],
                            None);
    }

    term
}

fn mark_cut_variables_as(terms: &mut Vec<Term>, name: ClauseName) {
    for term in terms.iter_mut() {
        match term {
            &mut Term::Constant(_, Constant::Atom(ref mut var)) if var.as_str() == "!" =>
                *var = name.clone(),
            _ => {}
        }
    }
}

fn mark_cut_variable(term: &mut Term) -> bool {
    let cut_var_found = match term {
        &mut Term::Constant(_, Constant::Atom(ref var)) if var.as_str() == "!" => true,
        _ => false
    };

    if cut_var_found {
        *term = Term::Var(Cell::default(), rc_atom!("!"));
        true
    } else {
        false
    }
}

fn mark_cut_variables(terms: &mut Vec<Term>) -> bool {
    let mut found_cut_var = false;

    for item in terms.iter_mut() {
        found_cut_var = mark_cut_variable(item);
    }

    found_cut_var
}

fn module_resolution_call(mod_name: Term, body: Term) -> Result<QueryTerm, ParserError> {
    if let Term::Constant(_, Constant::Atom(mod_name)) = mod_name {
        if let Term::Clause(_, name, terms, _) = body {
            let idx = CodeIndex(Rc::new(RefCell::new((IndexPtr::Module, mod_name))));
            return Ok(QueryTerm::Clause(Cell::default(), ClauseType::Named(name, idx), terms,
                                        false));
        }
    }

    Err(ParserError::InvalidModuleResolution)
}

pub enum TopLevelPacket {
    Query(Vec<QueryTerm>, Vec<TopLevel>),
    Decl(TopLevel, Vec<TopLevel>)
}

struct RelationWorker {
    queue: VecDeque<VecDeque<Term>>
}

impl RelationWorker {
    fn new() -> Self {
        RelationWorker { queue: VecDeque::new() }
    }

    fn compute_head(&self, term: &Term) -> Vec<Term>
    {
        let mut vars = HashSet::new();

        for term in post_order_iter(term) {
            if let TermRef::Var(_, _, v) = term {
                vars.insert(v.clone());
            }
        }

        vars.insert(rc_atom!("!"));
        vars.into_iter()
            .map(|v| Term::Var(Cell::default(), v))
            .collect()
    }

    fn fabricate_rule_body(&self, vars: &Vec<Term>, body_term: Term) -> Term
    {
        let vars_of_head = vars.iter().cloned().map(Box::new).collect();
        let head_term = Term::Clause(Cell::default(), clause_name!(""), vars_of_head, None);

        let rule = vec![Box::new(head_term), Box::new(body_term)];
        let turnstile = clause_name!(":-");

        Term::Clause(Cell::default(), turnstile, rule, None)
    }

    // the terms form the body of the rule. We create a head, by
    // gathering variables from the body of terms and recording them
    // in the head clause.
    fn fabricate_rule(&self, body_term: Term) -> (JumpStub, VecDeque<Term>)
    {
        // collect the vars of body_term into a head, return the num_vars
        // (the arity) as well.
        let vars = self.compute_head(&body_term);
        let rule = self.fabricate_rule_body(&vars, body_term);

        (vars, VecDeque::from(vec![rule]))
    }

    fn fabricate_disjunct(&self, body_term: Term) -> (JumpStub, VecDeque<Term>)
    {
        let mut cut_var_found = false;

        let mut vars = self.compute_head(&body_term);
        let clauses: Vec<_> = unfold_by_str(body_term, ";").into_iter()
            .map(|term| {
                let mut subterms = unfold_by_str(term, ",");
                cut_var_found = mark_cut_variables(&mut subterms);

                let term = subterms.pop().unwrap();
                fold_by_str(subterms, term, clause_name!(","))
            }).collect();

        if cut_var_found {
            vars.push(Term::Var(Cell::default(), rc_atom!("!")));
        }

        let results = clauses.into_iter()
            .map(|clause| self.fabricate_rule_body(&vars, clause))
            .collect();

        (vars, results)
    }

    fn fabricate_if_then(&self, prec: Term, conq: Term) -> (JumpStub, VecDeque<Term>)
    {
        let mut prec_seq = unfold_by_str(prec, ",");
        let comma_sym    = clause_name!(",");
        let cut_sym      = atom!("!");

        prec_seq.push(Term::Constant(Cell::default(), cut_sym));

        mark_cut_variables_as(&mut prec_seq, clause_name!("blocked_!"));

        let mut conq_seq = unfold_by_str(conq, ",");

        mark_cut_variables(&mut conq_seq);
        prec_seq.extend(conq_seq.into_iter());

        let back_term  = Box::new(prec_seq.pop().unwrap());
        let front_term = Box::new(prec_seq.pop().unwrap());

        let body_term  = Term::Clause(Cell::default(), comma_sym.clone(),
                                      vec![front_term, back_term], None);

        self.fabricate_rule(fold_by_str(prec_seq, body_term, comma_sym))
    }

    fn to_query_term(&mut self, indices: &mut MachineCodeIndices, term: Term) -> Result<QueryTerm, ParserError>
    {
        match term {
            Term::Constant(r, Constant::Atom(name)) =>
                if name.as_str() == "!" || name.as_str() == "blocked_!" {
                    Ok(QueryTerm::BlockedCut)
                } else {
                    let ct = indices.lookup(name, 0, None);
                    Ok(QueryTerm::Clause(r, ct, vec![], false))
                },
            Term::Var(_, ref v) if v.as_str() == "!" =>
                Ok(QueryTerm::UnblockedCut(Cell::default())),
            Term::Clause(r, name, mut terms, fixity) =>
                match (name.as_str(), terms.len()) {
                    (";", 2) => {
                        let term = Term::Clause(r, name.clone(), terms, fixity);
                        let (stub, clauses) = self.fabricate_disjunct(term);

                        self.queue.push_back(clauses);
                        Ok(QueryTerm::Jump(stub))
                    },
                    (":", 2) => {
                        let callee   = *terms.pop().unwrap();
                        let mod_name = *terms.pop().unwrap();

                        module_resolution_call(mod_name, callee)
                    },
                    ("->", 2) => {
                        let conq = *terms.pop().unwrap();
                        let prec = *terms.pop().unwrap();

                        let (stub, clauses) = self.fabricate_if_then(prec, conq);

                        self.queue.push_back(clauses);
                        Ok(QueryTerm::Jump(stub))
                    },
                    ("$get_level", 1) =>
                        if let Term::Var(_, ref var) = *terms[0] {
                            Ok(QueryTerm::GetLevelAndUnify(Cell::default(), var.clone()))
                        } else {
                            Err(ParserError::InadmissibleQueryTerm)
                        },
                    ("partial_string", 2) => {
                        if let Term::Constant(_, Constant::String(_)) = *terms[0].clone() {
                            if let Term::Var(..) = *terms[1].clone() {
                                let ct = ClauseType::BuiltIn(BuiltInClauseType::PartialString);
                                return Ok(QueryTerm::Clause(Cell::default(), ct, terms, false));
                            }
                        }

                        Err(ParserError::InadmissibleQueryTerm)
                    },
                    _ => {
                        let ct = indices.lookup(name, terms.len(), fixity);
                        Ok(QueryTerm::Clause(Cell::default(), ct, terms, false))
                    }
                },
            Term::Var(..) =>
                Ok(QueryTerm::Clause(Cell::default(), ClauseType::CallN, vec![Box::new(term)], false)),
            _ => Err(ParserError::InadmissibleQueryTerm)
        }
    }

    // never blocks cuts in the consequent.
    fn prepend_if_then(&self, prec: Term, conq: Term, queue: &mut VecDeque<Box<Term>>,
                       blocks_cuts: bool)
    {
        let cut_symb = atom!("blocked_!");
        let mut terms_seq = unfold_by_str(prec, ",");

        terms_seq.push(Term::Constant(Cell::default(), cut_symb));

        let mut conq_seq = unfold_by_str(conq, ",");

        if !blocks_cuts {
            for item in conq_seq.iter_mut() {
                mark_cut_variable(item);
            }
        }

        terms_seq.append(&mut conq_seq);

        while let Some(term) = terms_seq.pop() {
            queue.push_front(Box::new(term));
        }
    }

    fn pre_query_term(&mut self, idx: &mut MachineCodeIndices, term: Term)
                      -> Result<QueryTerm, ParserError>
    {
        match term {
            Term::Clause(r, name, mut subterms, fixity) =>
                if subterms.len() == 1 && name.as_str() == "$call_with_default_policy" {
                    self.to_query_term(idx, *subterms.pop().unwrap())
                        .map(|mut query_term| {
                            query_term.set_default_caller();
                            query_term
                        })
                } else {
                    self.to_query_term(idx, Term::Clause(r, name, subterms, fixity))
                },
            _ => self.to_query_term(idx, term)
        }
    }

    fn setup_query(&mut self, idx: &mut MachineCodeIndices, terms: Vec<Box<Term>>, blocks_cuts: bool)
                   -> Result<Vec<QueryTerm>, ParserError>
    {
        let mut query_terms = vec![];
        let mut work_queue  = VecDeque::from(terms);

        while let Some(term) = work_queue.pop_front() {
            let mut term = *term;

            // a (->) clause makes up the entire query. That's what the test confirms.
            if query_terms.is_empty() && work_queue.is_empty() {
                // check for ->, inline it if found.
                if let &mut Term::Clause(_, ref name, ref mut subterms, _) = &mut term {
                    if name.as_str() == "->" && subterms.len() == 2 {
                        let conq = *subterms.pop().unwrap();
                        let prec = *subterms.pop().unwrap();

                        self.prepend_if_then(prec, conq, &mut work_queue, blocks_cuts);
                        continue;
                    }
                }
            }

            for mut subterm in unfold_by_str(term, ",") {
                if !blocks_cuts {
                    mark_cut_variable(&mut subterm);
                }

                query_terms.push(self.pre_query_term(idx, subterm)?);
            }
        }

        Ok(query_terms)
    }

    fn setup_rule(&mut self, idx: &mut MachineCodeIndices, mut terms: Vec<Box<Term>>, blocks_cuts: bool)
                  -> Result<Rule, ParserError>
    {
        let post_head_terms = terms.drain(1..).collect();
        let mut query_terms = try!(self.setup_query(idx, post_head_terms, blocks_cuts));
        let clauses = query_terms.drain(1 ..).collect();
        let qt = query_terms.pop().unwrap();

        match *terms.pop().unwrap() {
            Term::Clause(_, name, terms, _) =>
                Ok(Rule { head: (name, terms, qt), clauses }),
            Term::Constant(_, Constant::Atom(name)) =>
                Ok(Rule { head: (name, vec![], qt), clauses }),
            _ => Err(ParserError::InvalidRuleHead)
        }
    }

    fn try_term_to_tl(&mut self, idx: &mut MachineCodeIndices, term: Term, blocks_cuts: bool)
                      -> Result<TopLevel, ParserError>
    {
        match term {
            Term::Clause(r, name, mut terms, fixity) =>
                if name.as_str() == "?-" {
                    Ok(TopLevel::Query(try!(self.setup_query(idx, terms, blocks_cuts))))
                } else if name.as_str() == ":-" && terms.len() > 1 {
                    Ok(TopLevel::Rule(try!(self.setup_rule(idx, terms, blocks_cuts))))
                } else if name.as_str() == ":-" && terms.len() == 1 {
                    let term = *terms.pop().unwrap();
                    Ok(TopLevel::Declaration(try!(setup_declaration(term))))
                } else {
                    Ok(TopLevel::Fact(try!(setup_fact(Term::Clause(r, name, terms, fixity)))))
                },
            term => Ok(TopLevel::Fact(try!(setup_fact(term))))
        }
    }

    fn try_terms_to_tls<Iter>(&mut self, idx: &mut MachineCodeIndices, terms: Iter, blocks_cuts: bool)
                              -> Result<VecDeque<TopLevel>, ParserError>
        where Iter: IntoIterator<Item=Term>
    {
        let mut results = VecDeque::new();

        for term in terms.into_iter() {
            results.push_back(self.try_term_to_tl(idx, term, blocks_cuts)?);
        }

        Ok(results)
    }

    fn parse_queue(&mut self, idx: &mut MachineCodeIndices) -> Result<VecDeque<TopLevel>, ParserError>
    {
        let mut queue = VecDeque::new();

        while let Some(terms) = self.queue.pop_front() {
            let clauses = merge_clauses(&mut self.try_terms_to_tls(idx, terms, false)?)?;
            queue.push_back(clauses);
        }

        Ok(queue)
    }

    fn absorb(&mut self, other: RelationWorker) {
        self.queue.extend(other.queue.into_iter());
    }
}

pub struct TopLevelWorker<'a, R: Read> {
    pub parser: Parser<R>,
    indices: MachineCodeIndices<'a>
}

impl<'a, R: Read> TopLevelWorker<'a, R> {
    pub fn new(inner: R, atom_tbl: TabledData<Atom>, flags: MachineFlags,
               indices: MachineCodeIndices<'a>)
               -> Self
    {
        TopLevelWorker { parser: Parser::new(inner, atom_tbl, flags), indices }
    }

    pub fn parse_code(&mut self) -> Result<TopLevelPacket, ParserError>
    {
        let mut rel_worker = RelationWorker::new();

        let terms   = self.parser.read(self.indices.op_dir)?;
        let mut tls = rel_worker.try_terms_to_tls(&mut self.indices, terms, true)?;
        let results = rel_worker.parse_queue(&mut self.indices)?;

        let tl = merge_clauses(&mut tls)?;

        if tls.is_empty() {
            Ok(deque_to_packet(tl, results))
        } else {
            Err(ParserError::InconsistentEntry)
        }
    }
}

pub struct TopLevelBatchWorker<R: Read> {
    parser: Parser<R>,
    rel_worker: RelationWorker,
    pub source_mod: ClauseName,
    pub results: Vec<(Predicate, VecDeque<TopLevel>)>
}

impl<R: Read> TopLevelBatchWorker<R> {
    pub fn new(inner: R, atom_tbl: TabledData<Atom>, flags: MachineFlags) -> Self
    {
        TopLevelBatchWorker { parser: Parser::new(inner, atom_tbl, flags),
                              rel_worker: RelationWorker::new(),
                              source_mod: clause_name!("user"),
                              results: vec![] }
    }

    pub
    fn consume(&mut self, indices: &mut MachineCodeIndices) -> Result<Option<Declaration>, SessionError>
    {
        let mut preds = vec![];

        while !self.parser.eof() {
            self.parser.reset(); // empty the parser stack of token descriptions.

            let mut new_rel_worker = RelationWorker::new();
            let term = self.parser.read_term(&indices.op_dir)?;
            let tl = new_rel_worker.try_term_to_tl(indices, term, true)?;

            if !is_consistent(&tl, &preds) {  // if is_consistent returns false, preds is non-empty.
                let result_queue = self.rel_worker.parse_queue(indices)?;
                self.results.push((append_preds(&mut preds), result_queue));
            }

            self.rel_worker.absorb(new_rel_worker);

            match tl {
                TopLevel::Fact(fact) => preds.push(PredicateClause::Fact(fact)),
                TopLevel::Rule(rule) => preds.push(PredicateClause::Rule(rule)),
                TopLevel::Predicate(pred) => preds.extend(pred.0),
                TopLevel::Declaration(decl) => return Ok(Some(decl)),
                TopLevel::Query(_) => return Err(SessionError::NamelessEntry)
            }
        }

        if !preds.is_empty() {
            let result_queue = self.rel_worker.parse_queue(indices)?;
            self.results.push((append_preds(&mut preds), result_queue));
        }

        Ok(None)
    }
}
