//! One Cartesian product shared by constructors and boundary dispatch.

#[macro_export]
macro_rules! book_policy_matrix {
    ($callback:ident $(, $argument:tt)*) => {
        $crate::__book_policy_matrix!(@users $callback ($($argument),*) [] [
            (Users, true, $crate::UpdateUserMap)
            (NoUsers, false, $crate::DoNotUpdateUserMap)
        ]);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __book_policy_matrix {
    (@users $callback:ident $args:tt [$($out:tt)*] []) => {
        $callback! { $args [$($out)*] }
    };
    (@users $callback:ident $args:tt $out:tt [$user:tt $($users:tt)*]) => {
        $crate::__book_policy_matrix!(@hidden $callback $args $out $user [$($users)*] [
            (Hidden, true, $crate::policies::UpdateHiddenQuantity)
            (NoHidden, false, $crate::policies::DoNotUpdateHiddenQuantity)
        ]);
    };
    (@hidden $callback:ident $args:tt $out:tt $user:tt $users:tt []) => {
        $crate::__book_policy_matrix!(@users $callback $args $out $users);
    };
    (@hidden $callback:ident $args:tt $out:tt $user:tt $users:tt [$hidden:tt $($rest:tt)*]) => {
        $crate::__book_policy_matrix!(@checksum $callback $args $out $user $users $hidden [$($rest)*] [
            (Null, $crate::policies::checksum::NoChecksum)
            (BitFinex, $crate::policies::checksum::BitFinex)
            (Kraken, $crate::policies::checksum::Kraken)
        ]);
    };
    (@checksum $callback:ident $args:tt $out:tt $user:tt $users:tt $hidden:tt $rest:tt []) => {
        $crate::__book_policy_matrix!(@hidden $callback $args $out $user $users $rest);
    };
    (@checksum $callback:ident $args:tt [$($out:tt)*]
        ($u:ident, $ub:literal, $ut:ty) $users:tt
        ($h:ident, $hb:literal, $ht:ty) $rest:tt [($c:ident, $ct:ty) $($checks:tt)*]) => {
        $crate::__book_policy_matrix!(@checksum $callback $args
            [$($out)* ($u, $ub, $ut, $h, $hb, $ht, $c, $ct),]
            ($u, $ub, $ut) $users ($h, $hb, $ht) $rest [$($checks)*]);
    };
}
