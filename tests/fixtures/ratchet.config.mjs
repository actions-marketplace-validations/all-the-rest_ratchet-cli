// A JavaScript config: the same shape as the JSON config, as a default export.
// The config is data only — a rule counts through `command` or `file`.
export default [
  {
    name: "fixture",
    rules: {
      "js-rule": {
        class: "layer violation",
        incident: "a rule was configured in JavaScript",
        unit: "call sites",
        direction: "ceiling",
        tier: "fast",
        count: { command: ["echo", "1"] },
      },
    },
  },
];
