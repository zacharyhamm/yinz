/** Public command output before and after a private extraction. */
export function before(name: string) {
  return `Hello, ${name.trim()}!`;
}
function cleanName(name: string) {
  return name.trim();
}
export function after(name: string) {
  return `Hello, ${cleanName(name)}!`;
}
