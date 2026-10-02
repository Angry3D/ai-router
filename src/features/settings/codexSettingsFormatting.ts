export function formatDateTime(value: number | null) {
  return value === null
    ? "-"
    : new Intl.DateTimeFormat("zh-CN", {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(value);
}
