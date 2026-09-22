import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SearchForm } from "./SearchForm";

describe("SearchForm", () => {
  it("search_form_rejects_invalid_coordinates", () => {
    const onSearch = vi.fn();
    render(<SearchForm isLoading={false} onSearch={onSearch} />);
    fireEvent.change(screen.getByLabelText("Destination address"), {
      target: { value: "Marszałkowska 10" },
    });
    fireEvent.change(screen.getByLabelText("Origin latitude"), { target: { value: "91" } });
    fireEvent.change(screen.getByLabelText("Origin longitude"), { target: { value: "21" } });
    fireEvent.click(screen.getByRole("button", { name: "Search parking" }));

    expect(screen.getByRole("alert")).toHaveTextContent("Latitude must be a number between -90 and 90.");
    expect(onSearch).not.toHaveBeenCalled();
  });

  it("search_form_submits_valid_request", () => {
    const onSearch = vi.fn();
    render(<SearchForm isLoading={false} onSearch={onSearch} />);
    fireEvent.change(screen.getByLabelText("Destination address"), {
      target: { value: "  Marszałkowska 10  " },
    });
    fireEvent.change(screen.getByLabelText("Origin latitude"), { target: { value: "52.2297" } });
    fireEvent.change(screen.getByLabelText("Origin longitude"), { target: { value: "21.0122" } });
    fireEvent.click(screen.getByRole("button", { name: "Search parking" }));

    expect(onSearch).toHaveBeenCalledWith({
      origin: { latitude: 52.2297, longitude: 21.0122 },
      destination_address: "Marszałkowska 10",
    });
  });
});
