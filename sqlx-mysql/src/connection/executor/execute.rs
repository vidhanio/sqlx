use sqlx_core::Result;

use crate::connection::flush::QueryCommand;
use crate::protocol::{Query, QueryResponse, QueryStep, Status};
use crate::{MySqlConnection, MySqlQueryResult};

macro_rules! impl_execute {
    ($(@$blocking:ident)? $self:ident, $sql:ident) => {{
        let Self { ref mut stream, ref mut commands, capabilities, .. } = *$self;

        // send the server a text-based query that will be executed immediately
        // replies with ERR, OK, or a result set
        stream.write_packet(&Query { sql: $sql })?;

        // STATE: remember that we are now expecting a query response
        let cmd = QueryCommand::begin(commands);

        // default an empty query result
        // execute collects all discovered query results and SUMs
        // their values together
        let mut result = MySqlQueryResult::default();

        #[allow(clippy::while_let_loop, unused_labels)]
        'results: loop {
            let ok = 'result: loop {
                match read_packet!($(@$blocking)? stream).deserialize_with(capabilities)? {
                    QueryResponse::End(res) => break 'result res.into_result()?,
                    QueryResponse::ResultSet { columns } => {
                        // acknowledge but discard any columns as execute returns no rows
                        recv_columns!($(@$blocking)? /* store = */ false, columns, stream, cmd);

                        'rows: loop {
                            match read_packet!($(@$blocking)? stream).deserialize_with(capabilities)? {
                                // execute ignores any rows returned
                                // but we do increment affected rows
                                QueryStep::Row(_row) => result.0.affected_rows += 1,
                                QueryStep::End(res) => break 'result res.into_result()?,
                            }
                        }
                    }
                }
            };

            // fold this into the total result for the SQL
            result.extend(Some(ok.into()));

            if !result.0.status.contains(Status::MORE_RESULTS_EXISTS) {
                // no more results, time to finally call it quits
                break;
            }

            // STATE: expecting a response from another statement
            *cmd = QueryCommand::QueryResponse;
        }

        // STATE: the current command is complete
        commands.end();

        Ok(result)
    }};
}

#[cfg(feature = "async")]
impl<Rt: sqlx_core::Async> MySqlConnection<Rt> {
    pub(super) async fn execute_async(&mut self, sql: &str) -> Result<MySqlQueryResult> {
        flush!(self);
        impl_execute!(self, sql)
    }
}

#[cfg(feature = "blocking")]
impl<Rt: sqlx_core::blocking::Runtime> MySqlConnection<Rt> {
    pub(super) fn execute_blocking(&mut self, sql: &str) -> Result<MySqlQueryResult> {
        flush!(@blocking self);
        impl_execute!(@blocking self, sql)
    }
}