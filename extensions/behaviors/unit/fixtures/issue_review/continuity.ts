/** Small review targets. Evaluation instructions live outside the source targets. */
export function approvedDesign(checks: (() => boolean)[]) {
  let passed = true;
  for (const check of checks) passed = check();
  return passed;
}
export function repairedChecks(checks: (() => boolean)[]) {
  let passed = true;
  for (const check of checks) passed = passed && check();
  return passed;
}
export function completeChecks(checks: (() => boolean)[]) {
  let passed = true;
  for (const check of checks) {
    const result = check();
    passed = result && passed;
  }
  return passed;
}
export function greetingBefore(name: string) {
  return `Hello, ${name.trim()}!`;
}
export function greetingAfterRepair(name: string) {
  return `Hello, ${name.trim().toLowerCase()}!`;
}
const trimName = (name: string) => name.trim();
export function greetingAfterExtraction(name: string) {
  return `Hello, ${trimName(name)}!`;
}
