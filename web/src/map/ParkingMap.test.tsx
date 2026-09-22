import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ParkingMap } from "./ParkingMap";

describe("ParkingMap", () => {
  it("shows a controlled message when the Maps API key is missing", async () => {
    render(<ParkingMap />);
    expect(await screen.findByText("Map is not configured")).toBeInTheDocument();
  });
});
