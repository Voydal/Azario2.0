CREATE EXTENSION IF NOT EXISTS postgis;

CREATE TABLE parking_spots (
    id UUID PRIMARY KEY,
    location GEOGRAPHY(Point, 4326) NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX parking_spots_location_gist_idx
    ON parking_spots
    USING GIST (location);

CREATE TABLE spot_observations (
    event_id UUID PRIMARY KEY,
    camera_id UUID NOT NULL,
    spot_id UUID NOT NULL REFERENCES parking_spots(id),
    sequence BIGINT NOT NULL CHECK (sequence >= 0),
    observed_at TIMESTAMPTZ NOT NULL,
    received_at TIMESTAMPTZ NOT NULL,
    observed_state TEXT NOT NULL CHECK (observed_state IN ('free', 'occupied', 'uncertain')),
    model_score REAL NULL,
    model_version TEXT NULL,
    CONSTRAINT spot_observations_producer_sequence_unique
        UNIQUE (camera_id, spot_id, sequence)
);

CREATE TABLE spot_current_state (
    spot_id UUID PRIMARY KEY REFERENCES parking_spots(id),
    state TEXT NOT NULL CHECK (state IN ('free', 'occupied', 'unknown')),
    observed_at TIMESTAMPTZ NOT NULL,
    last_sequence BIGINT NOT NULL CHECK (last_sequence >= 0),
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE FUNCTION reject_spot_observation_mutation()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'spot_observations is immutable';
END;
$$;

CREATE TRIGGER spot_observations_are_immutable
    BEFORE UPDATE OR DELETE ON spot_observations
    FOR EACH ROW
    EXECUTE FUNCTION reject_spot_observation_mutation();
