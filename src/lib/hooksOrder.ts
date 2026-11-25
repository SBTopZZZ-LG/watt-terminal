import * as ts from 'typescript';

/*
 * The rules of hooks, checked on source text. A component (a function named with a capital letter) or a hook (`use` + capital)
 * must call every hook on every render, in the same order. A hook after an early `return` in the same function, or inside an
 * `if`/`else`, a loop, a `switch`, a `try`, or the right side of `&&`/`||`/`??` or a `?:` branch, breaks that: React throws
 * "Rendered fewer hooks than expected". Hooks in nested functions (callbacks) are not checked. `use(...)` (React 19's `use` API)
 * is not a hook here, so it may be called after a return.
 */

export interface HookOrderProblem {
  /** 1-based line of the hook call. */
  line: number;
  /** The component or hook the call is in. */
  owner: string;
  /** The hook called, e.g. `useRef` or `React.useState`. */
  hook: string;
  reason: 'after-return' | 'conditional';
}

const HOOK = /^use[A-Z]/;
const CHECKED = /^([A-Z]|use[A-Z])/;
const WRAPPER = /^(memo|forwardRef)$/;
const LOGICAL = new Set<ts.SyntaxKind>([ts.SyntaxKind.AmpersandAmpersandToken, ts.SyntaxKind.BarBarToken, ts.SyntaxKind.QuestionQuestionToken]);

/** The hook a call makes (`useX(...)` or `React.useX(...)`), or null for any other call. */
function hookName(call: ts.CallExpression): string | null {
  const callee = call.expression;
  if (ts.isIdentifier(callee) && HOOK.test(callee.text)) {
    return callee.text;
  }
  if (ts.isPropertyAccessExpression(callee) && ts.isIdentifier(callee.expression) && callee.expression.text === 'React' && HOOK.test(callee.name.text)) {
    return `React.${callee.name.text}`;
  }
  return null;
}

/** The hook calls under a node, not counting those in nested functions. */
function hookCalls(root: ts.Node): ts.CallExpression[] {
  const found: ts.CallExpression[] = [];
  const visit = (node: ts.Node): void => {
    if (ts.isFunctionLike(node)) {
      return;
    }
    if (ts.isCallExpression(node) && hookName(node) !== null) {
      found.push(node);
    }
    ts.forEachChild(node, visit);
  };
  visit(root);
  return found;
}

/** Whether a return statement is under a node, not counting nested functions. */
function containsReturn(root: ts.Node): boolean {
  let found = false;
  const visit = (node: ts.Node): void => {
    if (found || ts.isFunctionLike(node)) {
      return;
    }
    if (ts.isReturnStatement(node)) {
      found = true;
      return;
    }
    ts.forEachChild(node, visit);
  };
  visit(root);
  return found;
}

/** Whether a call sits where it may not run on every render, between it and the function body. */
function isConditional(call: ts.Node, body: ts.Node): boolean {
  let child: ts.Node = call;
  let parent: ts.Node | undefined = call.parent;
  while (parent !== undefined && child !== body) {
    if (ts.isIfStatement(parent) && child !== parent.expression) {
      return true;
    }
    if (ts.isConditionalExpression(parent) && child !== parent.condition) {
      return true;
    }
    if (ts.isBinaryExpression(parent) && child === parent.right && LOGICAL.has(parent.operatorToken.kind)) {
      return true;
    }
    if (ts.isIterationStatement(parent, false) || ts.isSwitchStatement(parent) || ts.isTryStatement(parent) || ts.isCatchClause(parent)) {
      return true;
    }
    child = parent;
    parent = parent.parent;
  }
  return false;
}

/** The function a component or hook is written as: a function, or one wrapped in memo() or forwardRef(). */
function componentFunction(expr: ts.Expression): ts.FunctionLikeDeclaration | null {
  let e: ts.Expression = expr;
  for (;;) {
    if (ts.isParenthesizedExpression(e)) {
      e = e.expression;
    } else if (ts.isCallExpression(e) && e.arguments.length > 0) {
      const callee = e.expression;
      const name = ts.isIdentifier(callee) ? callee.text : ts.isPropertyAccessExpression(callee) ? callee.name.text : '';
      if (!WRAPPER.test(name)) {
        return null;
      }
      e = e.arguments[0];
    } else {
      break;
    }
  }
  return ts.isArrowFunction(e) || ts.isFunctionExpression(e) ? e : null;
}

function checkFunction(sf: ts.SourceFile, owner: string, fn: ts.FunctionLikeDeclaration, out: HookOrderProblem[]): void {
  const body = fn.body;
  if (body === undefined) {
    return;
  }
  const report = (call: ts.CallExpression, reason: HookOrderProblem['reason']) => {
    const line = sf.getLineAndCharacterOfPosition(call.getStart(sf)).line + 1;
    out.push({ line, owner, hook: hookName(call) ?? '', reason });
  };
  const afterReturn = new Set<ts.CallExpression>();
  if (ts.isBlock(body)) {
    let returned = false;
    for (const stmt of body.statements) {
      if (returned) {
        for (const call of hookCalls(stmt)) {
          afterReturn.add(call);
          report(call, 'after-return');
        }
      }
      returned = returned || containsReturn(stmt);
    }
  }
  for (const call of hookCalls(body)) {
    if (!afterReturn.has(call) && isConditional(call, body)) {
      report(call, 'conditional');
    }
  }
}

/** The hook-order problems in one source file. `fileName` picks the parser (.tsx or .ts). */
export function findHookOrderProblems(fileName: string, text: string): HookOrderProblem[] {
  const kind = /\.tsx$/.test(fileName) ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
  const sf = ts.createSourceFile(fileName, text, ts.ScriptTarget.Latest, true, kind);
  const out: HookOrderProblem[] = [];
  const visit = (node: ts.Node): void => {
    if (ts.isFunctionDeclaration(node) && node.name && CHECKED.test(node.name.text)) {
      checkFunction(sf, node.name.text, node, out);
    }
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && CHECKED.test(node.name.text) && node.initializer) {
      const fn = componentFunction(node.initializer);
      if (fn !== null) {
        checkFunction(sf, node.name.text, fn, out);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return out;
}
