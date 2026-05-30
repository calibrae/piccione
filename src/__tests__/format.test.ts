import { describe, it, expect } from "vitest";
import { parseFormatting, parseMentions } from "../lib/format";

describe("parseFormatting", () => {
  it("plain text yields no ranges", () => {
    const { text, ranges } = parseFormatting("hello world");
    expect(text).toBe("hello world");
    expect(ranges).toEqual([]);
  });

  it("bold", () => {
    const { text, ranges } = parseFormatting("a **bold** b");
    expect(text).toBe("a bold b");
    expect(ranges).toEqual([{ start: 2, length: 4, style: "bold" }]);
  });

  it("italic with single star", () => {
    const { text, ranges } = parseFormatting("*hi*");
    expect(text).toBe("hi");
    expect(ranges).toEqual([{ start: 0, length: 2, style: "italic" }]);
  });

  it("strikethrough, spoiler, monospace", () => {
    expect(parseFormatting("~~x~~").ranges[0].style).toBe("strikethrough");
    expect(parseFormatting("||x||").ranges[0].style).toBe("spoiler");
    expect(parseFormatting("`x`").ranges[0].style).toBe("monospace");
  });

  it("multiple ranges with correct offsets on cleaned text", () => {
    const { text, ranges } = parseFormatting("**A** and *B*");
    expect(text).toBe("A and B");
    expect(ranges).toEqual([
      { start: 0, length: 1, style: "bold" },
      { start: 6, length: 1, style: "italic" },
    ]);
  });

  it("unmatched marker is left as literal", () => {
    const { text, ranges } = parseFormatting("a ** b");
    expect(text).toBe("a ** b");
    expect(ranges).toEqual([]);
  });
});

  describe("parseMentions", () => {
    const members = [
      { uuid: "u-bob", name: "Bob" },
      { uuid: "u-bobsmith", name: "Bob Smith" },
    ];
    it("matches a member name", () => {
      const r = parseMentions("hi @Bob", members);
      expect(r).toEqual([{ start: 3, length: 4, mention_uuid: "u-bob" }]);
    });
    it("longest name wins, no overlap", () => {
      const r = parseMentions("@Bob Smith here", members);
      expect(r).toEqual([{ start: 0, length: 10, mention_uuid: "u-bobsmith" }]);
    });
    it("no match yields no ranges", () => {
      expect(parseMentions("nobody here", members)).toEqual([]);
    });
  });
