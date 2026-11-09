/** The command scheduler returns an aggregate result for the caller's merge gate. */
export function runChecks(checks: (() => boolean)[]) {
  let passed = true;
  const executed: number[] = [];
  for (const [index, check] of checks.entries()) {
    passed = check();
    executed.push(index);
  }
  return { passed, executed };
}

/** Existing regression assertion used by this fixture's approved plan. */
export function existingRegression() {
  return runChecks([() => false, () => true]).executed.length === 2;
}
