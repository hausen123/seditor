(define (factorial n)
  (if (< n 1)
      #f
      (let loop ((count n) (acc 1))
        (if (<= count 1) acc (loop (- count 1) (* count acc))))))

(define (power x n)
  (if (and (eqv? x 0) (eqv? n 0))
      #f
      (let loop ((count n) (acc 1))
        (cond ((eqv? count 0) acc)
              ((> count 0) (loop (- count 1) (* acc x)))
              (else (loop (+ count 1) (/ acc x)))))))

(define (power2 x n)
  (if (and (eqv? x 0) (<= n 0))
      #f
      (let ((loop (lambda (loop count acc)
                    (cond ((eqv? count 0) acc)
                          ((> count 0) (loop loop (- count 1) (* acc x)))
                          (else (loop loop (+ count 1) (/ acc x)))))))
        (loop loop n 1))))

(define (collatz n)
  (let loop ((x n) (result ()))
    (cond ((eqv? x 1) (reverse (cons x result)))
          (else (loop (if (even? x) (/ x 2) (+ (* 3 x) 1)) (cons x result))))))

(define (test lis)
  (let loop ((lis lis) (result ()))
    (if (eq? lis ()) result (loop (cdr lis) (cons (car lis) result)))))

(define (pack lis)
  (let loop ((lis lis) (result '()))
    (if (null? lis)
        (reverse result)
        (let ((result_new (if (null? result)
                              (list (list (car lis)))
                              (if (equal? (caar result) (car lis))
                                  (cons (cons (car lis) (car result))
                                        (cdr result))
                                  (cons (list (car lis)) result)))))
          (loop (cdr lis) result_new)))))

(define (encode lis)
  (map (lambda (elm) (list (length elm) (car elm))) (pack lis)))

(define (bind lis)
  (let loop ((lis (reverse lis)) (result '()))
    (if (null? lis)
        result
        (let ((x (car lis)))
          (if (pair? x)
              (loop (cdr lis) (append x result))
              (loop (cdr lis) (cons x result)))))))

(define (nested-list? xs)
  (cond ((not (pair? xs)) #f)
        ((pair? (car xs)) #t)
        (else (nested-list? (cdr xs)))))

(define (flatten lis)
  (bind (map (lambda (l) (if (nested-list? l) (flatten l) l)) lis)))

(define (calc lis)
  (let ((ope (cond ((eqv? (car lis) '+) +)
                   ((eqv? (car lis) '-) -)
                   ((eqv? (car lis) '*) *)
                   ((eqv? (car lis) '/) /)
                   (else #f)))
        (x1 (cadr lis))
        (x2 (caddr lis)))
    (if (not (eqv? ope #f))
        (ope (if (number? x1) x1 (calc x1)) (if (number? x2) x2 (calc x2)))
        #f)))

(let ((lis '(* 2 (/ 3 4))))
  (format #t "calc\(lis = ~a\) = ~a\n" lis (calc lis)))

(let ((n 4))
  (format #t "factorial\(n = ~a\) = ~a\n" n (factorial n)))

(let ((x 2) (n -3))
  (format #t "power\(x = ~a, n = ~a\) = ~a\n" x n (power x n)))

(let ((x -2) (n -2))
  (format #t "power2\(x = ~a, n = ~a\) = ~a\n" x n (power2 x n)))

(let ((n 100))
  (format #t "collatz\(n = ~a\) = ~a\n" n (collatz n)))

(let ((lis '(1 2 2 3)))
  (format #t "pack\(lis = ~a\) = ~a\n" lis (pack lis)))

(let ((lis '(a b b b c c C)))
  (format #t "encode\(lis = ~a\) = ~a\n" lis (encode lis)))

(let ((lis '((1 2 3) 4)))
  (format #t "bind\(lis = ~a\) = ~a\n" lis (bind lis)))

(let ((lis '(((1 2 3) 4) (5 y))))
  (format #t "flatten\(lis = ~a\) = ~a\n" lis (flatten lis)))
