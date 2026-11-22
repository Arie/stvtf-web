import js from "@eslint/js";
import globals from "globals";

export default [
  { ignores: ["app/vendor/"] },
  js.configs.recommended,
  {
    files: ["app/**/*.js", "test/**/*.js", "script/**/*.mjs"],
    languageOptions: { ecmaVersion: "latest", sourceType: "module", globals: { ...globals.browser, ...globals.node } },
    rules: {
      "no-unused-vars": ["error", { args: "none", caughtErrors: "none" }],
      eqeqeq: ["error", "smart"],
      "no-var": "error",
      "prefer-const": "error",
      "no-shadow": "error",
      "no-empty": ["error", { allowEmptyCatch: true }],
      "no-implicit-coercion": "error",
      "no-unneeded-ternary": "error",
      "no-useless-return": "error",
      "object-shorthand": "error",
      "no-nested-ternary": "error",
    },
  },
  // Limits on the size and shape of the page's code. What broke them when they were added is listed
  // in eslint-suppressions.json: a file may not add more, and what is fixed is pruned with
  // `npx eslint app test script --prune-suppressions`.
  {
    files: ["app/**/*.js"],
    rules: {
      complexity: ["error", 15],
      "max-depth": ["error", 4],
      "max-params": ["error", 5],
      "max-lines-per-function": ["error", { max: 60, skipBlankLines: true, skipComments: true }],
      "max-lines": ["error", { max: 400, skipBlankLines: true, skipComments: true }],
    },
  },
];
