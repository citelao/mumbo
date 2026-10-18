import { describe, expect, it } from "vitest";
import { z } from "zod";
import { User } from "./mumbleState";
// Real serde output; a Rust test asserts this file matches what Rust emits.
import fixture from "../crates/mumble-protocol/fixtures/users.json";

describe("User", () => {
  it("parses Rust's users", () => {
    const result = z.array(User).safeParse(fixture);
    expect(result.error?.issues).toBeUndefined();
  });

  it("rejects fields the schema doesn't know about", () => {
    const user = { ...fixture[0], new_field: 1 };
    expect(User.safeParse(user).success).toBe(false);
  });

  it("rejects missing fields", () => {
    const { name: _name, ...user } = fixture[0];
    expect(User.safeParse(user).success).toBe(false);
  });
});
