import js from "@eslint/js";
import globals from "globals";

export default [
  js.configs.recommended,
  {
    files: ["app/**/*.js"],
    languageOptions: { ecmaVersion: "latest", sourceType: "module", globals: globals.browser },
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
    },
  },
];
