import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { successResponse } from "../test/fixtures";
import { SearchResult } from "./SearchResult";

describe("SearchResult", () => {
  it("search_result_displays_drive_and_walk_metrics", () => {
    render(<SearchResult response={successResponse} />);
    expect(screen.getByText("1.8 km · 4 min 35 s")).toBeInTheDocument();
    expect(screen.getByText("180 m · 2 min 25 s")).toBeInTheDocument();
    expect(screen.getByText("7 min")).toBeInTheDocument();
  });

  it("walking_beta_warning_is_rendered", () => {
    render(<SearchResult response={successResponse} />);
    expect(screen.getByText(/Walking routes are beta/)).toBeInTheDocument();
  });

  it("renders_no_available_parking_as_a_normal_result", () => {
    render(<SearchResult response={{ result: null, reason: "no_available_parking" }} />);
    expect(screen.getByText(/No fresh, available parking spots/)).toBeInTheDocument();
  });

  it("renders_no_reachable_parking_as_a_normal_result", () => {
    render(<SearchResult response={{ result: null, reason: "no_reachable_parking" }} />);
    expect(screen.getByText(/no complete driving and walking route/)).toBeInTheDocument();
  });
});
